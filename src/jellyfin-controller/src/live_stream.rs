use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use jellyfin_model::MediaSourceInfo;
use uuid::Uuid;

const DEFAULT_MAX_OPEN_STREAMS: usize = 256;
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveStreamRegistryError {
    CapacityExceeded,
    ScopeMismatch,
}

#[derive(Clone, Debug)]
struct OpenLiveStream {
    item_id: Uuid,
    media_source: MediaSourceInfo,
    consumer_count: usize,
    last_access: Instant,
}

/// In-memory registry for media sources returned by `/LiveStreams/Open`.
///
/// Official source providers may return the same opened stream to multiple
/// consumers. Each open increments its consumer count and each close releases
/// one consumer; the source stops resolving only after the last close.
#[derive(Clone, Debug)]
pub struct LiveStreamRegistry {
    streams: Arc<Mutex<HashMap<String, OpenLiveStream>>>,
    max_open_streams: usize,
    idle_timeout: Duration,
}

impl LiveStreamRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: Arc::default(),
            max_open_streams: DEFAULT_MAX_OPEN_STREAMS,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// Registers an opened source or reuses the existing source with the same id.
    ///
    /// A follower reusing the same stream id does not consume another capacity
    /// slot. New streams are rejected once the bounded registry is full.
    pub fn open(
        &self,
        item_id: Uuid,
        media_source: MediaSourceInfo,
    ) -> Result<MediaSourceInfo, LiveStreamRegistryError> {
        let Some(live_stream_id) = media_source
            .live_stream_id
            .as_deref()
            .filter(|value| !value.is_empty())
        else {
            return Ok(media_source);
        };
        // The official MediaSourceManager stores open streams in a
        // StringComparer.OrdinalIgnoreCase dictionary. Live-stream ids are
        // provider-generated ASCII identifiers, so normalize the registry
        // key while preserving the original wire value in MediaSourceInfo.
        let registry_key = live_stream_id.to_ascii_lowercase();
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_expired(&mut streams, self.idle_timeout);
        if let Some(open) = streams.get_mut(&registry_key) {
            if open.item_id != item_id {
                return Err(LiveStreamRegistryError::ScopeMismatch);
            }
            open.consumer_count = open.consumer_count.saturating_add(1);
            open.last_access = Instant::now();
            return Ok(open.media_source.clone());
        }
        if streams.len() >= self.max_open_streams {
            return Err(LiveStreamRegistryError::CapacityExceeded);
        }
        streams.insert(
            registry_key,
            OpenLiveStream {
                item_id,
                media_source: media_source.clone(),
                consumer_count: 1,
                last_access: Instant::now(),
            },
        );
        Ok(media_source)
    }

    /// Returns a response-safe clone of an opened media source.
    #[must_use]
    pub fn get(&self, item_id: Uuid, live_stream_id: &str) -> Option<MediaSourceInfo> {
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_expired(&mut streams, self.idle_timeout);
        let open = streams.get_mut(&live_stream_id.to_ascii_lowercase())?;
        if open.item_id != item_id {
            return None;
        }
        open.last_access = Instant::now();
        Some(open.media_source.clone())
    }

    /// Releases one consumer and removes the source after its final close.
    ///
    /// The official close endpoint is idempotent for unknown stream ids, so a
    /// missing id is reported as `false` rather than treated as an error.
    pub fn close(&self, live_stream_id: &str) -> bool {
        let registry_key = live_stream_id.to_ascii_lowercase();
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_expired(&mut streams, self.idle_timeout);
        let Some(open) = streams.get_mut(&registry_key) else {
            return false;
        };
        if open.consumer_count > 1 {
            open.consumer_count -= 1;
        } else {
            streams.remove(&registry_key);
        }
        true
    }
}

impl Default for LiveStreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn prune_expired(streams: &mut HashMap<String, OpenLiveStream>, idle_timeout: Duration) {
    streams.retain(|_, stream| stream.last_access.elapsed() < idle_timeout);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jellyfin_model::MediaSourceInfo;
    use uuid::Uuid;

    use super::{LiveStreamRegistry, LiveStreamRegistryError};

    fn registry(max_open_streams: usize, idle_timeout: Duration) -> LiveStreamRegistry {
        LiveStreamRegistry {
            streams: std::sync::Arc::default(),
            max_open_streams,
            idle_timeout,
        }
    }

    #[test]
    fn repeated_open_reuses_source_until_last_consumer_closes() {
        let registry = LiveStreamRegistry::new();
        let item_id = Uuid::new_v4();
        let first = MediaSourceInfo {
            live_stream_id: Some("live-1".to_owned()),
            path: Some("/media/first.ts".to_owned()),
            ..MediaSourceInfo::default()
        };
        let replacement = MediaSourceInfo {
            live_stream_id: Some("live-1".to_owned()),
            path: Some("/media/replacement.ts".to_owned()),
            ..MediaSourceInfo::default()
        };

        let _ = registry.open(item_id, first).unwrap();
        let reused = registry.open(item_id, replacement).unwrap();
        assert_eq!(reused.path.as_deref(), Some("/media/first.ts"));

        assert!(registry.close("LIVE-1"));
        assert!(registry.get(item_id, "LiVe-1").is_some());
        assert!(registry.close("live-1"));
        assert!(registry.get(item_id, "live-1").is_none());
        assert!(!registry.close("live-1"));
    }

    #[test]
    fn lookups_are_item_scoped_and_followers_do_not_consume_capacity() {
        let registry = registry(1, Duration::from_secs(60));
        let item_id = Uuid::new_v4();
        let other_item_id = Uuid::new_v4();
        let source = MediaSourceInfo {
            live_stream_id: Some("live-1".to_owned()),
            ..MediaSourceInfo::default()
        };

        registry.open(item_id, source.clone()).unwrap();
        registry.open(item_id, source).unwrap();
        assert!(registry.get(item_id, "live-1").is_some());
        assert!(registry.get(other_item_id, "live-1").is_none());

        let second = MediaSourceInfo {
            live_stream_id: Some("live-2".to_owned()),
            ..MediaSourceInfo::default()
        };
        assert_eq!(
            registry.open(item_id, second),
            Err(LiveStreamRegistryError::CapacityExceeded)
        );
    }

    #[test]
    fn expired_entries_are_pruned_before_capacity_is_checked() {
        let registry = registry(1, Duration::ZERO);
        let item_id = Uuid::new_v4();
        registry
            .open(
                item_id,
                MediaSourceInfo {
                    live_stream_id: Some("expired".to_owned()),
                    ..MediaSourceInfo::default()
                },
            )
            .unwrap();
        registry
            .open(
                item_id,
                MediaSourceInfo {
                    live_stream_id: Some("replacement".to_owned()),
                    ..MediaSourceInfo::default()
                },
            )
            .unwrap();

        assert!(registry.get(item_id, "expired").is_none());
    }
}
