use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    BaseItemQuery, BaseItemRepository, ResumePageId, resumable_filtered_cte,
    resumable_page_id_query,
};
use crate::DatabaseConfig;

/// Run explicitly against the test PostgreSQL administrator, optionally setting
/// JELLYFIN_RESUME_PLAN_DUMP to preserve both complete EXPLAIN JSON documents.
/// The fixture owns a fresh database; work_mem changes are transaction-local.
#[tokio::test]
#[ignore = "creates an isolated PostgreSQL database for the resume memory comparison"]
async fn resume_materialization_plan() {
    let config = DatabaseConfig::default();
    let administrator = crate::connect(&config)
        .await
        .expect("PostgreSQL administrator");
    let database_name = format!("jellyfin_resume_plan_{}", Uuid::new_v4().simple());
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("create isolated plan database");
    let (url_prefix, _) = config
        .url
        .rsplit_once('/')
        .expect("PostgreSQL database URL");
    let test_config = DatabaseConfig {
        url: format!("{url_prefix}/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    };
    let outcome = tokio::spawn(async move { compare_resume_plans(test_config).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("drop isolated plan database");
    administrator
        .close()
        .await
        .expect("close administrator pool");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("plan comparison was cancelled: {error}");
    }
}

#[allow(clippy::too_many_lines)]
async fn compare_resume_plans(config: DatabaseConfig) {
    let database = crate::connect(&config)
        .await
        .expect("isolated plan database");
    crate::migrate(&database)
        .await
        .expect("plan fixture migrations");
    let user_id = Uuid::from_u128(101);
    seed_wide_resume_items(&database, user_id).await;
    let query = BaseItemQuery {
        user_id: Some(user_id),
        enable_all_folders: true,
        allowed_tags: vec!["visible".to_owned()],
        blocked_tags: vec!["blocked".to_owned()],
        start_index: 13,
        limit: Some(20),
        enable_total_record_count: Some(true),
        ..Default::default()
    };
    let (cte, values) = resumable_filtered_cte(user_id, &query);
    // The baseline is the previous production query. Share every filter, candidate CTE,
    // count, ordering and pagination clause so only the materialized projection differs.
    let baseline_cte = cte.replace(
        "SELECT item.id, candidate.resume_last_played_date",
        "SELECT item.*, candidate.resume_last_played_date",
    );
    assert_ne!(
        baseline_cte, cte,
        "baseline must retain the old wide projection"
    );
    let (baseline_sql, baseline_values) =
        resumable_page_id_query(baseline_cte, values.clone(), &query);
    let (current_sql, current_values) = resumable_page_id_query(cte, values, &query);
    let transaction = database.begin().await.expect("plan comparison transaction");
    transaction
        .execute_unprepared("SET LOCAL work_mem = '128kB'")
        .await
        .unwrap();
    let mut plans = Vec::new();
    let mut pages = Vec::new();
    for (label, sql, values) in [
        ("baseline", baseline_sql, baseline_values),
        ("current", current_sql, current_values),
    ] {
        // Fetching the actual page both verifies parity and warms the same query before EXPLAIN.
        let page = ResumePageId::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            values.clone(),
        ))
        .all(&transaction)
        .await
        .expect("real resume page");
        pages.push(
            page.into_iter()
                .map(|row| (row.total_record_count, row.id))
                .collect::<Vec<_>>(),
        );
        let plan = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}"),
                values,
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<Value>("", "QUERY PLAN")
            .unwrap();
        let filtered = find_filtered_plan(&plan[0]["Plan"]).expect("materialized filtered CTE");
        assert_eq!(
            filtered["Actual Rows"].as_u64(),
            Some(3600),
            "the memory comparison must materialize every policy-visible candidate"
        );
        let summary = json!({
            "label": label,
            "filtered_plan_width": filtered["Plan Width"],
            "filtered_actual_rows": filtered["Actual Rows"],
            "temp_read_blocks": plan[0]["Plan"]["Temp Read Blocks"],
            "temp_written_blocks": plan[0]["Plan"]["Temp Written Blocks"],
            "execution_time_ms": plan[0]["Execution Time"],
        });
        eprintln!("resume_plan {summary}");
        plans.push(json!({"summary": summary, "explain": plan}));
    }
    transaction.rollback().await.unwrap();
    assert_eq!(
        pages[0], pages[1],
        "count, order and pagination must be identical"
    );
    assert_eq!(pages[1].len(), 20);
    assert_eq!(pages[1][0].0, Some(3600));
    assert!(
        plans[1]["summary"]["filtered_plan_width"].as_u64().unwrap()
            < plans[0]["summary"]["filtered_plan_width"].as_u64().unwrap(),
        "the shared candidate tuplestore must carry narrower rows"
    );
    if let Some(path) = std::env::var_os("JELLYFIN_RESUME_PLAN_DUMP") {
        std::fs::write(path, serde_json::to_vec_pretty(&plans).unwrap())
            .expect("write plan report");
    }

    let repository = BaseItemRepository::new(database.clone());
    let page = repository.query_resumable(user_id, &query).await.unwrap();
    assert_eq!(page.total_record_count, 3600);
    assert_eq!(page.items.len(), 20);
    for (item, (_, expected_id)) in page.items.iter().zip(&pages[1]) {
        assert_eq!(Some(item.id), *expected_id);
        assert_eq!(item.data.as_ref().unwrap()["Tags"], json!(["visible"]));
        assert_eq!(
            item.data.as_ref().unwrap()["ProbeDescription"]
                .as_str()
                .unwrap()
                .len(),
            1280
        );
    }
    database.close().await.unwrap();
}

async fn seed_wide_resume_items(database: &DatabaseConnection, user_id: Uuid) {
    database.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO jellyfin.users (id, username, normalized_username) VALUES ($1, 'plan-user', 'PLAN-USER')",
        [user_id.into()],
    )).await.unwrap();
    // Keep a fixed, inline metadata payload so TOAST compression cannot hide the cost of
    // materializing complete rows. This storage setting affects only this throwaway fixture.
    database
        .execute_unprepared("ALTER TABLE jellyfin.base_items ALTER COLUMN data SET STORAGE PLAIN")
        .await
        .unwrap();
    database.execute_unprepared(
        "INSERT INTO jellyfin.base_items (id, item_type, name, sort_name, path, overview, data) \
         SELECT md5('resume-wide-' || value::text)::uuid, 'Movie', \
             'Resume movie ' || value::text, 'Resume movie ' || value::text, \
             '/library/' || repeat(md5(value::text), 8), repeat(md5(value::text), 16), \
             jsonb_build_object(\
                 'Tags', CASE WHEN value % 10 = 0 THEN jsonb_build_array('visible', 'blocked') \
                              ELSE jsonb_build_array('visible') END, \
                 'ProbeDescription', repeat(md5(value::text), 40)) \
         FROM generate_series(1, 4000) AS value",
    ).await.unwrap();
    // Match the persisted scan/editor contract: authorization filters normalized tag
    // relations, not the JSON copy returned as metadata. Keep fixture preparation set-based.
    database
        .execute_unprepared(
            "INSERT INTO jellyfin.item_values (item_value_id, type, value, clean_value) \
         VALUES (md5('resume-tag-visible')::uuid, 4, 'visible', 'visible'), \
                (md5('resume-tag-blocked')::uuid, 4, 'blocked', 'blocked'); \
         INSERT INTO jellyfin.item_value_map (item_value_id, item_id) \
         SELECT md5('resume-tag-visible')::uuid, md5('resume-wide-' || value::text)::uuid \
         FROM generate_series(1, 4000) AS value \
         UNION ALL \
         SELECT md5('resume-tag-blocked')::uuid, md5('resume-wide-' || value::text)::uuid \
         FROM generate_series(1, 4000) AS value WHERE value % 10 = 0",
        )
        .await
        .unwrap();
    database.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO jellyfin.user_data (item_id, user_id, custom_data_key, playback_position_ticks, last_played_date) \
         SELECT md5('resume-wide-' || value::text)::uuid, $1, '', 1000, \
             timestamptz '2026-01-01 00:00:00+00' + value * interval '1 second' \
         FROM generate_series(1, 4000) AS value",
        [user_id.into()],
    )).await.unwrap();
    database
        .execute_unprepared(
            "ANALYZE jellyfin.base_items; ANALYZE jellyfin.user_data; \
             ANALYZE jellyfin.item_values; ANALYZE jellyfin.item_value_map",
        )
        .await
        .unwrap();
}

fn find_filtered_plan(plan: &Value) -> Option<&Value> {
    if plan["Subplan Name"] == "CTE filtered" {
        return Some(plan);
    }
    plan["Plans"]
        .as_array()?
        .iter()
        .find_map(find_filtered_plan)
}
