use apple_music_tui::{
    app::{
        action::Action,
        reducer::reduce,
        state::{AppState, Route, Screen},
    },
    backend::capabilities::Capabilities,
    visualizer::{canvas::Canvas, config::Preset, effects::registry},
};

// Compile the copyable contributor module with the same crate path it uses
// after installation, so documentation cannot drift away from the public API.
mod visualizer {
    pub use apple_music_tui::visualizer::*;
}
#[path = "../docs/visualizers/pulse.rs"]
mod pulse;

#[test]
fn contributor_template_validates_parameters_and_responds_to_audio() {
    use visualizer::{analysis::AnalysisFrame, effects::Visualization};
    let mut effect = pulse::Pulse::default();
    let mut preset = Preset::default();
    preset
        .parameters
        .insert("radius".into(), toml::Value::Float(0.35));
    effect.validate(&preset).unwrap();
    effect.configure(&preset);
    let mut canvas = Canvas::default();
    canvas.resize(40, 20);
    effect.render(&mut canvas);
    let quiet = canvas.pixels().to_vec();
    effect.update(
        &AnalysisFrame {
            bass: 1.0,
            ..AnalysisFrame::default()
        },
        std::time::Duration::from_secs(1),
    );
    effect.render(&mut canvas);
    assert_ne!(quiet, canvas.pixels());
    preset
        .parameters
        .insert("radius".into(), toml::Value::Float(100.0));
    assert!(effect.validate(&preset).is_err());
}

#[test]
fn half_block_transport_preserves_both_pixel_colors_when_scaled() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};
    let mut canvas = Canvas::default();
    canvas.resize(1, 1);
    canvas.set(0, 0, [10, 20, 30]);
    canvas.set(0, 1, [40, 50, 60]);
    let area = Rect::new(0, 0, 2, 2);
    let mut buffer = Buffer::empty(area);
    canvas.present(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].fg, Color::Rgb(10, 20, 30));
    assert_eq!(buffer[(0, 1)].fg, Color::Rgb(40, 50, 60));
    assert_eq!(buffer[(1, 1)].bg, Color::Rgb(40, 50, 60));
}

#[test]
fn visualizer_preserves_navigation_and_keeps_playback_controls_available() {
    let mut state = AppState {
        content_selection: 7,
        capabilities: Capabilities::macos(),
        ..AppState::default()
    };
    state.navigation.active = Route::Section(Screen::Songs);
    let original = state.navigation.clone();
    reduce(&mut state, Action::ToggleVisualizer);
    reduce(&mut state, Action::CycleVisualizer);
    assert_eq!(state.visualizer_selection, 1);
    assert!(!reduce(&mut state, Action::PlayPause).is_empty());
    reduce(&mut state, Action::Back);
    assert!(!state.visualizer_open);
    assert_eq!(state.navigation, original);
    assert_eq!(state.content_selection, 7);
    assert!(!state.should_quit && !state.stop_playback_on_exit);
}

#[test]
fn every_registered_effect_handles_empty_and_small_canvases() {
    for mut effect in registry() {
        effect.configure(&Preset::default());
        let mut canvas = Canvas::default();
        effect.render(&mut canvas);
        for (w, h) in [(1, 1), (2, 1), (20, 5), (500, 300)] {
            canvas.resize(w, h);
            effect.render(&mut canvas);
            assert!(canvas.pixels().len() <= 240 * 128);
        }
    }
}

#[test]
fn shared_presets_are_valid_for_their_registered_effects() {
    for text in [include_str!("../examples/visualizer.toml")] {
        let settings: visualizer::config::Settings = toml::from_str(text).unwrap();
        settings.validate().unwrap();
        let effects = registry();
        for preset in settings.presets {
            let effect = effects.iter().find(|e| e.id() == preset.effect).unwrap();
            effect.validate(&preset).unwrap();
        }
    }
}
