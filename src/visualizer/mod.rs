pub mod analysis;
pub mod canvas;
pub mod config;
pub mod effects;
pub mod service;

use crate::{app::state::AppState, domain::PlaybackStatus};
use analysis::AnalysisFrame;
use canvas::Canvas;
use config::{Selection, Settings};
use effects::Visualization;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use service::{AudioService, CaptureStatus, Snapshot};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct Controller {
    settings: Settings,
    effects: Vec<Box<dyn Visualization>>,
    canvas: Canvas,
    service: AudioService,
    config_error: Option<String>,
    selection_path: Option<PathBuf>,
    initial: Selection,
    selected: (usize, usize),
    configured: bool,
    last_tick: Instant,
    last_retry: usize,
    snapshot: Snapshot,
    visible: bool,
}
impl Controller {
    pub fn load(allowed: bool) -> Self {
        let path = config::settings_path();
        let loaded = path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|p| Settings::load(p));
        let (settings, mut config_error) = match loaded {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e)),
        };
        let effects = effects::registry();
        for p in &settings.presets {
            if let Some(effect) = effects.iter().find(|e| e.id() == p.effect) {
                if let Err(error) = effect.validate(p) {
                    config_error = Some(format!("Preset '{}': {error}", p.name));
                }
            } else {
                config_error = Some(format!(
                    "Preset '{}' uses unknown effect '{}'.",
                    p.name, p.effect
                ));
            }
        }
        let selection_path = path.ok().map(|p| p.with_file_name("visualizer-state.toml"));
        let initial = selection_path
            .as_ref()
            .map(|p| config::load_selection(p))
            .unwrap_or_default();
        Self {
            service: AudioService::new(settings.backend, allowed),
            settings,
            effects,
            canvas: Canvas::default(),
            config_error,
            selection_path,
            initial,
            selected: (usize::MAX, usize::MAX),
            configured: false,
            last_tick: Instant::now(),
            last_retry: 0,
            snapshot: Snapshot::default(),
            visible: false,
        }
    }
    pub fn initialize(&self, state: &mut AppState) {
        let preset = self
            .settings
            .presets
            .iter()
            .position(|p| p.name == self.initial.preset)
            .or_else(|| {
                self.settings
                    .presets
                    .iter()
                    .position(|p| p.name == self.settings.initial_preset)
            })
            .unwrap_or(0);
        state.visualizer_preset = preset;
        state.visualizer_selection = self
            .effects
            .iter()
            .position(|e| e.id() == self.initial.effect)
            .or_else(|| {
                self.effects
                    .iter()
                    .position(|e| e.id() == self.settings.presets[preset].effect)
            })
            .unwrap_or(0);
    }
    pub fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.settings.framerate as f64)
    }
    pub fn sync(&mut self, state: &mut AppState) {
        let visible = state.visualizer_open && !state.help_open;
        let preset = state.visualizer_preset % self.settings.presets.len();
        if self.configured && preset != self.selected.1 {
            state.visualizer_selection = self
                .effects
                .iter()
                .position(|e| e.id() == self.settings.presets[preset].effect)
                .unwrap_or(0);
        }
        let effect = state.visualizer_selection % self.effects.len();
        if !self.configured || self.selected != (effect, preset) {
            self.effects[effect].configure(&self.settings.presets[preset]);
            self.selected = (effect, preset);
            self.configured = true;
        }
        if visible && !self.visible {
            self.effects[effect].reset();
            self.last_tick = Instant::now();
        }
        if !visible && self.visible {
            let _ = self.persist();
        }
        self.visible = visible;
        if state.visualizer_retry != self.last_retry {
            self.last_retry = state.visualizer_retry;
            self.service.retry();
        }
        self.service.set_enabled(
            visible
                && self.config_error.is_none()
                && state.playback.status == PlaybackStatus::Playing,
        );
        self.snapshot = self.service.snapshot();
        let audio = if self.snapshot.status == CaptureStatus::Live
            && self.snapshot.audio.captured_at.elapsed() < Duration::from_millis(500)
        {
            &self.snapshot.audio
        } else {
            &AnalysisFrame::default()
        };
        let elapsed = self.last_tick.elapsed().min(Duration::from_millis(150));
        self.last_tick = Instant::now();
        if visible && state.playback.status == PlaybackStatus::Playing {
            self.effects[effect].update(audio, elapsed);
        }
    }
    pub fn render(&mut self, frame: &mut Frame<'_>, state: &AppState) {
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().bg(Color::Rgb(2, 3, 10))),
            area,
        );
        let title = format!(
            " {} · {} ",
            self.effects[self.selected.0].name(),
            self.settings.presets[self.selected.1].name
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(96, 101, 171)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);
        let track = state
            .playback
            .current_track
            .as_ref()
            .map(|t| format!("{} · {}", t.title, t.artist))
            .unwrap_or_else(|| "Nothing playing".into());
        frame.render_widget(
            Paragraph::new(track).style(Style::default().fg(Color::Rgb(218, 223, 244))),
            rows[0],
        );
        let scope = match self.settings.backend {
            config::CaptureBackend::CoreAudio => "Music.app",
            config::CaptureBackend::ScreenCaptureKit => "System audio",
        };
        let status = if let Some(e) = &self.config_error {
            e.clone()
        } else if state.playback.status != PlaybackStatus::Playing {
            "Paused · capture stopped".into()
        } else {
            match &self.snapshot.status {
                CaptureStatus::Disabled | CaptureStatus::Initializing => "Starting capture…".into(),
                CaptureStatus::Live => format!("{scope} · Live"),
                CaptureStatus::Silent => {
                    "No local audio · check capture permission and output".into()
                }
                CaptureStatus::Unavailable(e) => e.clone(),
            }
        };
        frame.render_widget(
            Paragraph::new(status.as_str()).style(Style::default().fg(Color::Rgb(134, 151, 190))),
            rows[1],
        );
        self.canvas.resize(rows[2].width, rows[2].height);
        self.effects[self.selected.0].render(&mut self.canvas);
        self.canvas.present(rows[2], frame.buffer_mut());
        if self.config_error.is_some()
            || matches!(self.snapshot.status, CaptureStatus::Unavailable(_))
        {
            let message = Rect::new(
                rows[2].x.saturating_add(2),
                rows[2].y.saturating_add(1),
                rows[2].width.saturating_sub(4),
                rows[2].height.min(6),
            );
            frame.render_widget(
                Paragraph::new(status).wrap(Wrap { trim: true }).style(
                    Style::default()
                        .fg(Color::Rgb(247, 181, 156))
                        .bg(Color::Rgb(2, 3, 10)),
                ),
                message,
            );
        }
        frame.render_widget(
            Paragraph::new("Tab effect · V preset · c retry · Space pause · v / Esc back · ? help")
                .style(Style::default().fg(Color::Rgb(134, 151, 190))),
            rows[3],
        );
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn persist(&self) -> Result<(), String> {
        if !self.configured || self.config_error.is_some() {
            return Ok(());
        }
        if let Some(path) = &self.selection_path {
            config::save_selection(
                path,
                &Selection {
                    effect: self.effects[self.selected.0].id().into(),
                    preset: self.settings.presets[self.selected.1].name.clone(),
                },
            )?;
        }
        Ok(())
    }
    pub async fn finish(&mut self) {
        let _ = self.persist();
        self.service.finish().await;
    }
}
