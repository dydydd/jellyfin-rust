# Jellyfin Rust

Jellyfin 服务端的 Rust 重实现。项目以兼容官方 Jellyfin API 和 Web 客户端为目标，
使用 Axum、SeaORM、PostgreSQL 构建，当前处于持续开发阶段，尚未达到生产可用级别。

## 当前能力

- 用户、设备、API Key、认证与权限
- 媒体库扫描：电影、剧集、季/集、照片、书籍等类型解析，基于 `ffprobe` 提取媒体流与附件信息
- 增量扫描、并发扫描保护、文件系统监听
- 条目查询、搜索、筛选、收藏、播放状态、播放历史
- 播放列表、合集、艺术家人名/类型/年份等聚合页面
- 图片处理：缩略图、模糊哈希、图片上传/下载/交换
- 本地字幕、字体、媒体附件、Trickplay 清单
- SyncPlay、Quick Connect、Session 命令
- OpenAPI 文档：`/api-docs/openapi.json`
- TMDB 元数据：远程搜索、远程图片、电影/剧集深度刷新
- 命名解析、NFO/本地元数据、外部 ID 与外部链接
- Live TV 底层组件：XMLTV、Schedules Direct、HDHomeRun、录音元数据

## 架构

仓库是 Cargo workspace，按职责拆成多个 crate：

| Crate | 职责 |
| --- | --- |
| `jellyfin-api` | Axum 路由、HTTP 接口、OpenAPI 文档 |
| `jellyfin-controller` | 领域服务、元数据刷新、图片、SyncPlay 等 |
| `jellyfin-data` | PostgreSQL 仓储与实体 |
| `jellyfin-migration` | SeaORM 数据库迁移 |
| `jellyfin-model` | DTO、枚举、DLNA/串流模型 |
| `jellyfin-providers` | 元数据 Provider 的纯逻辑部分 |
| `jellyfin-naming` | 媒体文件命名解析 |
| `jellyfin-media-encoding` | ffprobe、编码参数、字幕转换 |
| `jellyfin-media-encoding-hls` | HLS 播放列表 |
| `jellyfin-media-encoding-keyframes` | 关键帧解析 |
| `jellyfin-drawing` | 图像处理与缩略图 |
| `jellyfin-extensions` | 通用工具扩展 |
| `jellyfin-networking` | 网络配置、代理解析 |
| `jellyfin-live-tv` | 直播电视、EPG、录音 |
| `jellyfin-server-implementations` | 会话、认证、同步播放等实现 |
| `jellyfin-xbmc-metadata` | XBMC/NFO 元数据读写 |
| `jellyfin-common` | 通用路径与 Provider ID 工具 |

## 快速开始

### 依赖

- Rust 1.88+
- PostgreSQL（本地默认连接 `postgres://postgres:123456@127.0.0.1:5432/postgres`）
- `ffprobe` 已加入 `PATH`，用于媒体库探测
- 可选：Jellyfin Web 构建产物，默认读取 `jellyfin-web/dist`

### 启动

```bash
cargo run -p jellyfin-server
```

默认监听 `127.0.0.1:8096`，首次启动会自动执行数据库迁移并创建初始管理员。

也可以显式指定数据库和监听地址：

```bash
DATABASE_URL=postgres://postgres:123456@127.0.0.1:5432/postgres \
JELLYFIN_BIND_ADDRESS=0.0.0.0:8096 \
cargo run -p jellyfin-server
```

## 配置

| 环境变量 | 说明 | 默认值 |
| --- | --- | --- |
| `DATABASE_URL` | PostgreSQL 连接串 | `postgres://postgres:123456@127.0.0.1:5432/postgres` |
| `JELLYFIN_BIND_ADDRESS` | HTTP 监听地址 | `127.0.0.1:8096` |
| `JELLYFIN_WEB_DIR` | Web 前端静态目录 | `jellyfin-web/dist` |
| `JELLYFIN_INITIAL_USER` | 首次启动创建的管理员用户名 | `jellyfin` |
| `JELLYFIN_TMDB_API_KEY` | TMDB API Key，可覆盖数据库配置 | 数据库中的 `tmdb_api_key` |
| `JELLYFIN_TMDB_PROXY` | TMDB HTTP 代理 | 未设置 |
| `JELLYFIN_FFMPEG_PATH` | 转码用 `FFmpeg` 可执行文件路径 | `ffmpeg` |

TMDB 请求也兼容标准的 `HTTPS_PROXY` / `ALL_PROXY`。例如：

```bash
JELLYFIN_TMDB_PROXY=http://127.0.0.1:7890 \
JELLYFIN_TMDB_API_KEY=your_key \
cargo run -p jellyfin-server
```

TMDB API Key 也可以通过 `/System/Configuration` 接口或管理界面写入数据库。

## Docker 本地部署

仓库提供 `compose.yaml`，会启动 Rust 服务和独立的 PostgreSQL 17 容器。首次启动时
服务会执行迁移并创建 `JELLYFIN_INITIAL_USER` 指定的管理员。默认管理员用户名为
`jellyfin`。

先确保可选的宿主机目录存在：

```bash
mkdir -p jellyfin-web/dist media
```

`jellyfin-web/dist` 应放入 Jellyfin Web 的构建产物；没有它时 API 仍可启动，但根页面
没有 Web UI。`media` 以只读方式挂载到容器的 `/media`，随后可在媒体库配置中使用该路径。

每次都使用全新的数据库和全新的 Jellyfin 运行时数据时，先删除本 Compose 项目的命名卷：

```bash
docker compose down --volumes --remove-orphans
docker compose up --build --force-recreate
```

第二条命令会在前台输出日志。Compose 默认使用宿主机 `18096`，避免与已有
Jellyfin/其他服务的 `8096` 冲突；启动成功后访问 `http://localhost:18096`，或检查 API：

```bash
curl http://localhost:18096/health
docker compose logs -f jellyfin
```

停止但保留数据使用 `docker compose down`。只有 `docker compose down --volumes` 会删除
PostgreSQL 数据库、程序数据、缓存和日志卷。

## 测试

```bash
# 编译整个 workspace
cargo check --workspace --all-targets

# 不依赖数据库的单元测试
cargo test -p jellyfin-controller --lib
```

部分集成测试需要本地 PostgreSQL，且测试会创建/删除以 `jellyfin_` 开头的临时数据库，
请确保连接账号拥有相应权限。

## 尚未完成

- 真正的 ffmpeg 转码任务编排；当前音频/视频流以直连文件为主，HLS 读取已有的转码产物
- Live TV 完整 API：目前仅接通 TunerHost 配置，频道、节目、录制、定时器等尚未接入
- 插件安装/卸载/配置管理
- 远程字幕、远程歌词、媒体分段 Provider
- MusicBrainz、AudioDb、ListenBrainz、书籍/漫画、照片等完整 Provider 集合
- 章节图、关键帧提取的调度接入
- 部分 WebSocket 事件推送

## License

与官方 Jellyfin 一致：[GNU General Public License v2.0](https://github.com/jellyfin/jellyfin/blob/master/LICENSE)

SPDX: `GPL-2.0-only`
