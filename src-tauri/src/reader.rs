use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use rosu_map::Beatmap as ParsedBeatmap;
use rosu_memory_lib::reader::{
    beatmap::BeatmapReader,
    common::{CommonReader, GameMode, GameState, OsuClientKind},
    gameplay::GameplayReader,
    resultscreen::ResultScreenReader,
    structs::State,
};
use rosu_mods::GameModsLegacy;
use rosu_pp::{
    any::{DifficultyAttributes, PerformanceAttributes, ScoreState},
    Beatmap, Difficulty, Performance,
};
use tauri::{AppHandle, Emitter};

use crate::{
    mock,
    models::{
        AppSnapshot, BeatmapSnapshot, ConnectionSnapshot, ConnectionStatus, HitSnapshot,
        LiveSnapshot, PpComponentSnapshot, PpSnapshot, RecentPlaySnapshot, SessionPhase,
        SessionSnapshot,
    },
    storage,
};

const SNAPSHOT_EVENT: &str = "session-updated";
const POLL_INTERVAL_MS: u64 = 90;
const INIT_RETRY_MS: u64 = 100;

pub fn spawn_live_reader(app: AppHandle, latest_snapshot: Arc<Mutex<Option<AppSnapshot>>>) {
    thread::spawn(move || {
        let mut recent_plays = storage::load_recent_plays(&app);
        emit_snapshot(
            &app,
            &latest_snapshot,
            mock::searching_snapshot_with_recent("Looking for osu!.exe.", recent_plays.clone()),
        );

        loop {
            let (mut state, process) = match rosu_memory_lib::init_loop(INIT_RETRY_MS) {
                Ok(result) => result,
                Err(error) => {
                    emit_snapshot(
                        &app,
                        &latest_snapshot,
                        mock::error_snapshot_with_recent(
                            format!("Unable to connect to osu!.exe: {error:?}"),
                            recent_plays.clone(),
                        ),
                    );
                    thread::sleep(Duration::from_millis(INIT_RETRY_MS));
                    continue;
                }
            };

            let mut cache = BeatmapCache::default();
            let mut last_result_signature: Option<String> = None;
            let mut last_gameplay_mods: Option<u32> = None;
            let mut gameplay_tracker = GameplayTracker::default();

            loop {
                match build_snapshot(
                    &process,
                    &mut state,
                    &mut cache,
                    &recent_plays,
                    &mut last_gameplay_mods,
                    &mut gameplay_tracker,
                ) {
                    Ok(mut snapshot) => {
                        if let Some(session) = snapshot.session.as_ref() {
                            maybe_push_recent_play(
                                &app,
                                &mut recent_plays,
                                &mut last_result_signature,
                                session,
                            );
                        }

                        snapshot.recent_plays = recent_plays.clone();
                        emit_snapshot(&app, &latest_snapshot, snapshot);
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    }
                    Err(error) => {
                        emit_snapshot(
                            &app,
                            &latest_snapshot,
                            mock::connected_snapshot_with_recent(
                                format!(
                                    "osu! detected, but live session data is unavailable: {error}"
                                ),
                                recent_plays.clone(),
                            ),
                        );
                        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    }
                }
            }
        }
    });
}

fn emit_snapshot(
    app: &AppHandle,
    latest_snapshot: &Arc<Mutex<Option<AppSnapshot>>>,
    snapshot: AppSnapshot,
) {
    if let Ok(mut guard) = latest_snapshot.lock() {
        *guard = Some(snapshot.clone());
    }

    let _ = app.emit(SNAPSHOT_EVENT, snapshot);
}

#[derive(Default)]
struct BeatmapCache {
    path: Option<PathBuf>,
    map: Option<Beatmap>,
    difficulty_by_mods: HashMap<u32, DifficultyAttributes>,
    bpm: Option<f64>,
    cover_path: Option<String>,
}

#[derive(Default)]
struct GameplayTracker {
    beatmap_path: Option<PathBuf>,
    retries: u32,
    previous_combo: u32,
    previous_misses: u32,
    previous_passed_objects: u32,
    slider_breaks: u32,
}

impl GameplayTracker {
    fn update(
        &mut self,
        mode: GameMode,
        beatmap_path: &Path,
        retries: u32,
        combo: u32,
        misses: u32,
        passed_objects: u32,
    ) -> u32 {
        let map_changed = self
            .beatmap_path
            .as_ref()
            .is_none_or(|current| current != beatmap_path);
        let play_restarted =
            map_changed || retries < self.retries || passed_objects < self.previous_passed_objects;

        if play_restarted {
            self.beatmap_path = Some(beatmap_path.to_path_buf());
            self.retries = retries;
            self.previous_combo = combo;
            self.previous_misses = misses;
            self.previous_passed_objects = passed_objects;
            self.slider_breaks = 0;
            return self.slider_breaks;
        }

        if mode == GameMode::Osu
            && passed_objects > self.previous_passed_objects
            && misses == self.previous_misses
            && self.previous_combo > 0
            && combo + 1 < self.previous_combo
        {
            self.slider_breaks = self.slider_breaks.saturating_add(1);
        }

        self.retries = retries;
        self.previous_combo = combo;
        self.previous_misses = misses;
        self.previous_passed_objects = passed_objects;

        self.slider_breaks
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl BeatmapCache {
    fn hydrate(&mut self, path: &Path, cover_filename: &str) -> Result<(), String> {
        let should_reload = self.path.as_ref().is_none_or(|cached| cached != path);

        if !should_reload {
            return Ok(());
        }

        let map = Beatmap::from_path(path).map_err(|error| error.to_string())?;
        let parsed_map = ParsedBeatmap::from_path(path).map_err(|error| error.to_string())?;

        let bpm = parsed_map
            .control_points
            .timing_points
            .first()
            .and_then(|point| (point.beat_len > 0.0).then_some(60_000.0 / point.beat_len));

        let cover_path = path.parent().and_then(|parent| {
            (!cover_filename.is_empty())
                .then_some(parent.join(cover_filename))
                .map(|p| p.display().to_string())
        });

        self.path = Some(path.to_path_buf());
        self.map = Some(map);
        self.bpm = bpm;
        self.cover_path = cover_path;
        self.difficulty_by_mods.clear();

        Ok(())
    }

    fn difficulty_for(
        &mut self,
        path: &Path,
        cover_filename: &str,
        mods: u32,
    ) -> Result<DifficultyAttributes, String> {
        self.hydrate(path, cover_filename)?;

        if let Some(cached) = self.difficulty_by_mods.get(&mods) {
            return Ok(cached.clone());
        }

        let map = self
            .map
            .as_ref()
            .ok_or_else(|| "Beatmap cache was not initialized".to_string())?;

        let difficulty = Difficulty::new().mods(mods).calculate(map);
        self.difficulty_by_mods.insert(mods, difficulty.clone());

        Ok(difficulty)
    }

    fn no_mod_difficulty(
        &mut self,
        path: &Path,
        cover_filename: &str,
    ) -> Result<DifficultyAttributes, String> {
        self.difficulty_for(path, cover_filename, 0)
    }
}

fn build_snapshot(
    process: &rosu_mem::process::Process,
    state: &mut State,
    cache: &mut BeatmapCache,
    recent_plays: &[RecentPlaySnapshot],
    last_gameplay_mods: &mut Option<u32>,
    gameplay_tracker: &mut GameplayTracker,
) -> Result<AppSnapshot, String> {
    let game_state = CommonReader::new(process, state, OsuClientKind::Stable)
        .game_state()
        .map_err(|error| error.to_string())?;

    let Some(phase) = phase_for_game_state(game_state) else {
        if game_state == GameState::MainMenu {
            if let Ok((beatmap_info, beatmap_path)) = read_beatmap_context(process, state) {
                if beatmap_path.exists() {
                    let session = build_preview_session(
                        process,
                        state,
                        cache,
                        game_state,
                        &beatmap_info,
                        &beatmap_path,
                    )?;

                    return Ok(AppSnapshot {
                        connection: ConnectionSnapshot {
                            status: ConnectionStatus::Connected,
                            detail: "Connected to osu!.exe · selected beatmap".to_string(),
                            updated_at_ms: mock::now_ms(),
                        },
                        session: Some(session),
                        recent_plays: recent_plays.to_vec(),
                    });
                }
            }
        }

        return Ok(connected_idle_snapshot(game_state, recent_plays));
    };

    let (beatmap_info, beatmap_path) = read_beatmap_context(process, state)?;

    let session = match phase {
        SessionPhase::Preview => build_preview_session(
            process,
            state,
            cache,
            game_state,
            &beatmap_info,
            &beatmap_path,
        )?,
        SessionPhase::Playing => build_playing_session(
            process,
            state,
            cache,
            game_state,
            &beatmap_info,
            &beatmap_path,
            last_gameplay_mods,
            gameplay_tracker,
        )?,
        SessionPhase::Result => build_result_session(
            process,
            state,
            cache,
            game_state,
            &beatmap_info,
            &beatmap_path,
            *last_gameplay_mods,
            gameplay_tracker.slider_breaks,
        )?,
    };

    if phase == SessionPhase::Preview {
        gameplay_tracker.reset();
    }

    Ok(AppSnapshot {
        connection: ConnectionSnapshot {
            status: ConnectionStatus::Connected,
            detail: format!(
                "Connected to osu!.exe · stable reader · {}",
                format_game_state(game_state)
            ),
            updated_at_ms: mock::now_ms(),
        },
        session: Some(session),
        recent_plays: recent_plays.to_vec(),
    })
}

fn maybe_push_recent_play(
    app: &AppHandle,
    recent_plays: &mut Vec<RecentPlaySnapshot>,
    last_result_signature: &mut Option<String>,
    session: &SessionSnapshot,
) {
    if session.phase != SessionPhase::Result {
        return;
    }

    let signature = format!(
        "{}:{}:{}:{}",
        session.beatmap.path, session.live.mods_text, session.live.score, session.live.combo
    );

    if last_result_signature.as_deref() == Some(signature.as_str()) {
        return;
    }

    *last_result_signature = Some(signature);

    recent_plays.insert(
        0,
        RecentPlaySnapshot {
            timestamp_ms: mock::now_ms(),
            title: format!(
                "{} - {} [{}]",
                session.beatmap.artist, session.beatmap.title, session.beatmap.difficulty_name
            ),
            mods_text: session.live.mods_text.clone(),
            accuracy: session.live.accuracy.unwrap_or_default(),
            combo: session.live.combo,
            pp: session.pp.current,
        },
    );

    recent_plays.truncate(5);

    if let Err(error) = storage::save_recent_plays(app, recent_plays) {
        log::warn!("Failed to persist recent plays: {error}");
    }
}

fn if_fc_state(state: &ScoreState, max_combo: u32) -> ScoreState {
    let mut next = state.clone();
    next.max_combo = max_combo;
    next.n300 += next.misses;
    next.misses = 0;
    next
}

fn component_breakdown(attrs: &PerformanceAttributes) -> Vec<PpComponentSnapshot> {
    match attrs {
        PerformanceAttributes::Osu(osu) => vec![
            component("Aim", osu.pp_aim),
            component("Speed", osu.pp_speed),
            component("Accuracy", osu.pp_acc),
            component(
                "Combo",
                osu.pp().max(0.0) - osu.pp_aim - osu.pp_speed - osu.pp_acc,
            ),
        ],
        PerformanceAttributes::Taiko(taiko) => vec![
            component("Aim", 0.0),
            component("Speed", taiko.pp_difficulty),
            component("Accuracy", taiko.pp_acc),
            component("Combo", 0.0),
        ],
        PerformanceAttributes::Catch(catch) => vec![
            component("Aim", catch.pp),
            component("Speed", 0.0),
            component("Accuracy", 0.0),
            component("Combo", 0.0),
        ],
        PerformanceAttributes::Mania(mania) => vec![
            component("Aim", 0.0),
            component("Speed", mania.pp_difficulty),
            component("Accuracy", 0.0),
            component("Combo", 0.0),
        ],
    }
}

fn component(label: &str, value: f64) -> PpComponentSnapshot {
    PpComponentSnapshot {
        label: label.to_string(),
        value: value.max(0.0),
    }
}

fn accuracy_for_mode(mode: GameMode, state: &ScoreState) -> f64 {
    match mode {
        GameMode::Osu => {
            let total = state.n300 + state.n100 + state.n50 + state.misses;

            if total == 0 {
                0.0
            } else {
                ((6 * state.n300 + 2 * state.n100 + state.n50) as f64 / (6 * total) as f64) * 100.0
            }
        }
        GameMode::Taiko => {
            let total = state.n300 + state.n100 + state.misses;

            if total == 0 {
                0.0
            } else {
                ((2 * state.n300 + state.n100) as f64 / (2 * total) as f64) * 100.0
            }
        }
        GameMode::Catch => {
            let total = state.n300 + state.n100 + state.n50 + state.n_katu + state.misses;

            if total == 0 {
                0.0
            } else {
                ((state.n300 + state.n100 + state.n50) as f64 / total as f64) * 100.0
            }
        }
        GameMode::Mania => {
            let total =
                state.n_geki + state.n300 + state.n_katu + state.n100 + state.n50 + state.misses;

            if total == 0 {
                0.0
            } else {
                ((6 * state.n_geki + 6 * state.n300 + 4 * state.n_katu + 2 * state.n100 + state.n50)
                    as f64
                    / (6 * total) as f64)
                    * 100.0
            }
        }
        GameMode::Unknown => 0.0,
    }
}

fn total_passed_objects(mode: GameMode, state: &ScoreState) -> u32 {
    match mode {
        GameMode::Osu => state.n300 + state.n100 + state.n50 + state.misses,
        GameMode::Taiko => state.n300 + state.n100 + state.misses,
        GameMode::Catch => state.n300 + state.n100 + state.n50 + state.n_katu + state.misses,
        GameMode::Mania => {
            state.n_geki + state.n300 + state.n_katu + state.n100 + state.n50 + state.misses
        }
        GameMode::Unknown => 0,
    }
}

fn format_game_state(state: GameState) -> String {
    match state {
        GameState::MainMenu => "Main menu",
        GameState::Editor => "Editor",
        GameState::Playing => "Playing",
        GameState::Exit => "Exit",
        GameState::EditorSongSelect => "Editor song select",
        GameState::SongSelect => "Song select",
        GameState::SelectDrawing => "Select drawing",
        GameState::ResultScreen => "Result screen",
        GameState::Update => "Update",
        GameState::Busy => "Busy",
        GameState::MultiplayerLobbySelect => "Multiplayer lobby select",
        GameState::MultiplayerLobby => "Multiplayer lobby",
        GameState::MultiplayerSongSelect => "Multiplayer song select",
        GameState::MultiplayerResultScreen => "Multiplayer result",
        GameState::OffsetWizard => "Offset wizard",
        GameState::MultiplayerResultScreenTagCoop => "Tag coop result",
        GameState::MultiplayerResultScreenTeamVs => "Team VS result",
        GameState::SongImport => "Song import",
        GameState::Unknown => "Unknown",
    }
    .to_string()
}

fn format_mods(bits: u32) -> String {
    let mods = GameModsLegacy::from_bits(bits).to_string();

    if mods.is_empty() {
        "NM".into()
    } else {
        mods
    }
}

fn phase_for_game_state(state: GameState) -> Option<SessionPhase> {
    match state {
        GameState::SongSelect | GameState::EditorSongSelect | GameState::MultiplayerSongSelect => {
            Some(SessionPhase::Preview)
        }
        GameState::Playing => Some(SessionPhase::Playing),
        GameState::ResultScreen
        | GameState::MultiplayerResultScreen
        | GameState::MultiplayerResultScreenTagCoop
        | GameState::MultiplayerResultScreenTeamVs => Some(SessionPhase::Result),
        _ => None,
    }
}

fn connected_idle_snapshot(
    game_state: GameState,
    recent_plays: &[RecentPlaySnapshot],
) -> AppSnapshot {
    let detail = if game_state == GameState::MainMenu {
        "osu! detected. Select a beatmap or start a map to see PP.".to_string()
    } else {
        format!(
            "osu! detected. {} is open; select a beatmap or start a map to see PP.",
            format_game_state(game_state)
        )
    };

    AppSnapshot {
        connection: ConnectionSnapshot {
            status: ConnectionStatus::Connected,
            detail,
            updated_at_ms: mock::now_ms(),
        },
        session: None,
        recent_plays: recent_plays.to_vec(),
    }
}

fn read_beatmap_context(
    process: &rosu_mem::process::Process,
    state: &mut State,
) -> Result<
    (
        rosu_memory_lib::reader::beatmap::common::BeatmapInfo,
        PathBuf,
    ),
    String,
> {
    let mut beatmap_reader = BeatmapReader::new(process, state, OsuClientKind::Stable)
        .map_err(|error| error.to_string())?;

    let beatmap_info = beatmap_reader.info().map_err(|error| error.to_string())?;
    let beatmap_path = beatmap_reader.path().map_err(|error| error.to_string())?;

    Ok((beatmap_info, beatmap_path))
}

fn build_preview_session(
    process: &rosu_mem::process::Process,
    state: &mut State,
    cache: &mut BeatmapCache,
    game_state: GameState,
    beatmap_info: &rosu_memory_lib::reader::beatmap::common::BeatmapInfo,
    beatmap_path: &Path,
) -> Result<SessionSnapshot, String> {
    let mods = CommonReader::new(process, state, OsuClientKind::Stable)
        .menu_game_mode()
        .unwrap_or_default();

    let difficulty = cache.difficulty_for(beatmap_path, &beatmap_info.location.cover, mods)?;
    let no_mod_difficulty = cache.no_mod_difficulty(beatmap_path, &beatmap_info.location.cover)?;
    let full_map_attrs = Performance::new(difficulty.clone())
        .lazer(false)
        .calculate();
    let no_mod_full_map_attrs = Performance::new(no_mod_difficulty.clone())
        .lazer(false)
        .calculate();

    Ok(SessionSnapshot {
        phase: SessionPhase::Preview,
        beatmap: build_beatmap_snapshot(cache, beatmap_info, beatmap_path, &difficulty, mods),
        live: LiveSnapshot {
            username: None,
            game_state: format_game_state(game_state),
            accuracy: None,
            combo: 0,
            max_combo: difficulty.max_combo(),
            score: 0,
            misses: 0,
            retries: 0,
            hp: None,
            progress: 0.0,
            passed_objects: 0,
            mods_text: format_mods(mods),
            hits: empty_hits(),
        },
        pp: build_pp_snapshot(
            full_map_attrs.pp(),
            full_map_attrs.pp(),
            full_map_attrs.pp(),
            &difficulty,
            &no_mod_difficulty,
            full_map_attrs.pp(),
            no_mod_full_map_attrs.pp(),
            component_breakdown(&full_map_attrs),
        ),
    })
}

fn build_playing_session(
    process: &rosu_mem::process::Process,
    state: &mut State,
    cache: &mut BeatmapCache,
    game_state: GameState,
    beatmap_info: &rosu_memory_lib::reader::beatmap::common::BeatmapInfo,
    beatmap_path: &Path,
    last_gameplay_mods: &mut Option<u32>,
    gameplay_tracker: &mut GameplayTracker,
) -> Result<SessionSnapshot, String> {
    let mut gameplay_reader = GameplayReader::new(process, state, OsuClientKind::Stable);
    let gameplay = gameplay_reader.info().map_err(|error| error.to_string())?;

    *last_gameplay_mods = Some(gameplay.mods);

    let difficulty =
        cache.difficulty_for(beatmap_path, &beatmap_info.location.cover, gameplay.mods)?;
    let no_mod_difficulty = cache.no_mod_difficulty(beatmap_path, &beatmap_info.location.cover)?;

    let score_state = ScoreState {
        max_combo: gameplay.combo.max(0) as u32,
        n_geki: gameplay_reader
            .hits_geki()
            .unwrap_or(gameplay.hits._geki)
            .max(0) as u32,
        n_katu: gameplay_reader
            .hits_katu()
            .unwrap_or(gameplay.hits._katu)
            .max(0) as u32,
        n300: gameplay.hits._300.max(0) as u32,
        n100: gameplay.hits._100.max(0) as u32,
        n50: gameplay.hits._50.max(0) as u32,
        misses: gameplay.hits._miss.max(0) as u32,
        legacy_total_score: Some(gameplay.score.max(0) as u32),
        ..ScoreState::new()
    };

    let current_passed_objects = total_passed_objects(beatmap_info.technical.mode, &score_state);
    let accuracy = accuracy_for_mode(beatmap_info.technical.mode, &score_state);
    let combo = gameplay.combo.max(0) as u32;
    let retries = gameplay.retries.max(0) as u32;
    let slider_breaks = gameplay_tracker.update(
        beatmap_info.technical.mode,
        beatmap_path,
        retries,
        combo,
        score_state.misses,
        current_passed_objects,
    );
    let partial_difficulty = Difficulty::new()
        .mods(gameplay.mods)
        .passed_objects(current_passed_objects.max(1))
        .calculate(
            cache
                .map
                .as_ref()
                .ok_or_else(|| "Beatmap cache was not initialized".to_string())?,
        );

    let (current_pp, components) = if current_passed_objects > 0 {
        let attrs = Performance::new(partial_difficulty.clone())
            .lazer(false)
            .state(score_state.clone())
            .passed_objects(current_passed_objects)
            .calculate();

        (attrs.pp(), component_breakdown(&attrs))
    } else {
        (0.0, zero_components())
    };

    let if_fc_state = if_fc_state(&score_state, partial_difficulty.max_combo());
    let if_fc_pp = Performance::new(partial_difficulty.clone())
        .lazer(false)
        .state(if_fc_state)
        .passed_objects(current_passed_objects.max(1))
        .calculate()
        .pp();
    let full_map_pp = Performance::new(difficulty.clone())
        .lazer(false)
        .calculate();
    let no_mod_full_map_pp = Performance::new(no_mod_difficulty.clone())
        .lazer(false)
        .calculate();

    let map_length_ms = beatmap_info.stats.length.max(0) as u32;
    let progress = if map_length_ms > 0 {
        (gameplay.ig_time.max(0) as f64 / map_length_ms as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    Ok(SessionSnapshot {
        phase: SessionPhase::Playing,
        beatmap: build_beatmap_snapshot(
            cache,
            beatmap_info,
            beatmap_path,
            &difficulty,
            gameplay.mods,
        ),
        live: LiveSnapshot {
            username: Some(gameplay.username),
            game_state: format_game_state(game_state),
            accuracy: Some(accuracy),
            combo,
            max_combo: difficulty.max_combo(),
            score: gameplay.score.max(0) as u32,
            misses: score_state.misses,
            retries,
            hp: Some(gameplay.hp.clamp(0.0, 100.0)),
            progress,
            passed_objects: current_passed_objects,
            mods_text: format_mods(gameplay.mods),
            hits: HitSnapshot {
                n_geki: score_state.n_geki,
                n_katu: score_state.n_katu,
                n300: score_state.n300,
                n100: score_state.n100,
                n50: score_state.n50,
                misses: score_state.misses,
                slider_breaks,
            },
        },
        pp: build_pp_snapshot(
            current_pp,
            if_fc_pp,
            full_map_pp.pp(),
            &difficulty,
            &no_mod_difficulty,
            full_map_pp.pp(),
            no_mod_full_map_pp.pp(),
            components,
        ),
    })
}

fn build_result_session(
    process: &rosu_mem::process::Process,
    state: &mut State,
    cache: &mut BeatmapCache,
    game_state: GameState,
    beatmap_info: &rosu_memory_lib::reader::beatmap::common::BeatmapInfo,
    beatmap_path: &Path,
    last_gameplay_mods: Option<u32>,
    tracked_slider_breaks: u32,
) -> Result<SessionSnapshot, String> {
    let result = ResultScreenReader::new(process, state, OsuClientKind::Stable)
        .info()
        .map_err(|error| error.to_string())?;

    let fallback_mods = CommonReader::new(process, state, OsuClientKind::Stable)
        .menu_game_mode()
        .unwrap_or_default();
    let mods = last_gameplay_mods.unwrap_or(fallback_mods);

    let difficulty = cache.difficulty_for(beatmap_path, &beatmap_info.location.cover, mods)?;
    let no_mod_difficulty = cache.no_mod_difficulty(beatmap_path, &beatmap_info.location.cover)?;

    let score_state = ScoreState {
        max_combo: result.max_combo.max(0) as u32,
        n_geki: result.hits._geki.max(0) as u32,
        n_katu: result.hits._katu.max(0) as u32,
        n300: result.hits._300.max(0) as u32,
        n100: result.hits._100.max(0) as u32,
        n50: result.hits._50.max(0) as u32,
        misses: result.hits._miss.max(0) as u32,
        legacy_total_score: Some(result.score.max(0) as u32),
        ..ScoreState::new()
    };

    let current_attrs = Performance::new(difficulty.clone())
        .lazer(false)
        .state(score_state.clone())
        .calculate();
    let if_fc_attrs = Performance::new(difficulty.clone())
        .lazer(false)
        .state(if_fc_state(&score_state, difficulty.max_combo()))
        .calculate();
    let full_map_attrs = Performance::new(difficulty.clone())
        .lazer(false)
        .calculate();
    let no_mod_full_map_attrs = Performance::new(no_mod_difficulty.clone())
        .lazer(false)
        .calculate();

    Ok(SessionSnapshot {
        phase: SessionPhase::Result,
        beatmap: build_beatmap_snapshot(cache, beatmap_info, beatmap_path, &difficulty, mods),
        live: LiveSnapshot {
            username: Some(result.username),
            game_state: format_game_state(game_state),
            accuracy: Some(result.accuracy),
            combo: result.max_combo.max(0) as u32,
            max_combo: difficulty.max_combo(),
            score: result.score.max(0) as u32,
            misses: score_state.misses,
            retries: 0,
            hp: None,
            progress: 1.0,
            passed_objects: total_passed_objects(beatmap_info.technical.mode, &score_state),
            mods_text: format_mods(mods),
            hits: HitSnapshot {
                n_geki: score_state.n_geki,
                n_katu: score_state.n_katu,
                n300: score_state.n300,
                n100: score_state.n100,
                n50: score_state.n50,
                misses: score_state.misses,
                slider_breaks: tracked_slider_breaks,
            },
        },
        pp: build_pp_snapshot(
            current_attrs.pp(),
            if_fc_attrs.pp(),
            full_map_attrs.pp(),
            &difficulty,
            &no_mod_difficulty,
            full_map_attrs.pp(),
            no_mod_full_map_attrs.pp(),
            component_breakdown(&current_attrs),
        ),
    })
}

fn build_beatmap_snapshot(
    cache: &BeatmapCache,
    beatmap_info: &rosu_memory_lib::reader::beatmap::common::BeatmapInfo,
    beatmap_path: &Path,
    difficulty: &DifficultyAttributes,
    mods: u32,
) -> BeatmapSnapshot {
    BeatmapSnapshot {
        artist: beatmap_info.metadata.author.clone(),
        title: beatmap_info.metadata.title_romanized.clone(),
        difficulty_name: beatmap_info.metadata.difficulty.clone(),
        creator: beatmap_info.metadata.creator.clone(),
        status: beatmap_info.technical.ranked_status.to_string(),
        mode: beatmap_info.technical.mode.to_string(),
        path: beatmap_path.display().to_string(),
        cover_path: cache.cover_path.clone(),
        length_ms: beatmap_info.stats.length.max(0) as u32,
        object_count: beatmap_info.stats.object_count.max(0) as u32,
        star_rating: difficulty.stars(),
        ar: beatmap_info.stats.ar as f64,
        od: beatmap_info.stats.od as f64,
        cs: beatmap_info.stats.cs as f64,
        hp: beatmap_info.stats.hp as f64,
        bpm: cache.bpm,
        mods: split_mods(mods),
    }
}

fn build_pp_snapshot(
    current: f64,
    if_fc: f64,
    full_map: f64,
    difficulty: &DifficultyAttributes,
    no_mod_difficulty: &DifficultyAttributes,
    modded_full_map: f64,
    no_mod_full_map: f64,
    components: Vec<PpComponentSnapshot>,
) -> PpSnapshot {
    let difficulty_adjust = if no_mod_difficulty.stars() > 0.0 {
        difficulty.stars() / no_mod_difficulty.stars()
    } else {
        1.0
    };

    let mods_multiplier = if no_mod_full_map > 0.0 {
        modded_full_map / no_mod_full_map
    } else {
        1.0
    };

    PpSnapshot {
        current,
        if_fc,
        full_map,
        calculator: "rosu-pp 4.0.1 · osu!stable scoring".into(),
        difficulty_adjust,
        mods_multiplier,
        components,
    }
}

fn empty_hits() -> HitSnapshot {
    HitSnapshot {
        n_geki: 0,
        n_katu: 0,
        n300: 0,
        n100: 0,
        n50: 0,
        misses: 0,
        slider_breaks: 0,
    }
}

fn zero_components() -> Vec<PpComponentSnapshot> {
    vec![
        component("Aim", 0.0),
        component("Speed", 0.0),
        component("Accuracy", 0.0),
        component("Combo", 0.0),
    ]
}

fn split_mods(bits: u32) -> Vec<String> {
    let mods = format_mods(bits);

    if mods == "NM" {
        Vec::new()
    } else {
        mods.as_bytes()
            .chunks(2)
            .filter_map(|chunk| std::str::from_utf8(chunk).ok())
            .map(ToOwned::to_owned)
            .collect()
    }
}
