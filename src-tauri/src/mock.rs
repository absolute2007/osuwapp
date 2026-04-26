use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::{AppSnapshot, ConnectionSnapshot, ConnectionStatus, RecentPlaySnapshot};

pub fn searching_snapshot_with_recent(
    detail: impl Into<String>,
    recent_plays: Vec<RecentPlaySnapshot>,
) -> AppSnapshot {
    status_snapshot(ConnectionStatus::Searching, detail.into(), recent_plays)
}

pub fn connected_snapshot_with_recent(
    detail: impl Into<String>,
    recent_plays: Vec<RecentPlaySnapshot>,
) -> AppSnapshot {
    status_snapshot(ConnectionStatus::Connected, detail.into(), recent_plays)
}

pub fn error_snapshot_with_recent(
    detail: impl Into<String>,
    recent_plays: Vec<RecentPlaySnapshot>,
) -> AppSnapshot {
    status_snapshot(ConnectionStatus::Error, detail.into(), recent_plays)
}

fn status_snapshot(
    status: ConnectionStatus,
    detail: String,
    recent_plays: Vec<RecentPlaySnapshot>,
) -> AppSnapshot {
    AppSnapshot {
        connection: ConnectionSnapshot {
            status,
            detail,
            updated_at_ms: now_ms(),
        },
        session: None,
        recent_plays,
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
