use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub connection: ConnectionSnapshot,
    pub session: Option<SessionSnapshot>,
    pub recent_plays: Vec<RecentPlaySnapshot>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlaySettings {
    pub enabled: bool,
    pub show_pp: bool,
    pub show_if_fc: bool,
    pub show_accuracy: bool,
    pub show_combo: bool,
    pub show_mods: bool,
    pub show_map: bool,
    pub show_hits: bool,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub scale: f64,
    pub font_scale: f64,
    pub padding: u32,
    pub corner_radius: u32,
    pub opacity: f64,
    pub show_background: bool,
    pub toggle_key: String,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            show_pp: true,
            show_if_fc: true,
            show_accuracy: true,
            show_combo: true,
            show_mods: true,
            show_map: true,
            show_hits: true,
            width: 420,
            height: 248,
            offset_x: 24,
            offset_y: 24,
            scale: 0.82,
            font_scale: 0.9,
            padding: 8,
            corner_radius: 12,
            opacity: 0.92,
            show_background: true,
            toggle_key: "Insert".to_string(),
        }
    }
}

impl OverlaySettings {
    pub fn normalized(mut self) -> Self {
        self.width = self.width.max(1);
        self.height = self.height.max(1);
        self.offset_x = self.offset_x.clamp(-3000, 3000);
        self.offset_y = self.offset_y.clamp(-3000, 3000);
        self.scale = self.scale.clamp(0.15, 2.5);
        self.font_scale = self.font_scale.clamp(0.45, 1.8);
        self.padding = self.padding.min(32);
        self.corner_radius = self.corner_radius.min(32);
        self.opacity = self.opacity.clamp(0.05, 1.0);
        self.toggle_key = normalize_toggle_key(&self.toggle_key);
        self
    }
}

fn normalize_toggle_key(value: &str) -> String {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        return "Insert".to_string();
    }

    trimmed.to_string()
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    Searching,
    Connected,
    Error,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSnapshot {
    pub status: ConnectionStatus,
    pub detail: String,
    pub updated_at_ms: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub phase: SessionPhase,
    pub beatmap: BeatmapSnapshot,
    pub live: LiveSnapshot,
    pub pp: PpSnapshot,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionPhase {
    Preview,
    Playing,
    Result,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatmapSnapshot {
    pub artist: String,
    pub title: String,
    pub difficulty_name: String,
    pub creator: String,
    pub status: String,
    pub mode: String,
    pub path: String,
    pub cover_path: Option<String>,
    pub length_ms: u32,
    pub object_count: u32,
    pub star_rating: f64,
    pub ar: f64,
    pub od: f64,
    pub cs: f64,
    pub hp: f64,
    pub bpm: Option<f64>,
    pub mods: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub username: Option<String>,
    pub game_state: String,
    pub accuracy: Option<f64>,
    pub combo: u32,
    pub max_combo: u32,
    pub score: u32,
    pub misses: u32,
    pub retries: u32,
    pub hp: Option<f64>,
    pub progress: f64,
    pub passed_objects: u32,
    pub mods_text: String,
    pub hits: HitSnapshot,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitSnapshot {
    pub n_geki: u32,
    pub n_katu: u32,
    pub n300: u32,
    pub n100: u32,
    pub n50: u32,
    pub misses: u32,
    pub slider_breaks: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpSnapshot {
    pub current: f64,
    pub if_fc: f64,
    pub full_map: f64,
    pub calculator: String,
    pub difficulty_adjust: f64,
    pub mods_multiplier: f64,
    pub components: Vec<PpComponentSnapshot>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpComponentSnapshot {
    pub label: String,
    pub value: f64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentPlaySnapshot {
    pub timestamp_ms: u64,
    pub title: String,
    pub mods_text: String,
    pub accuracy: f64,
    pub combo: u32,
    pub pp: f64,
}
