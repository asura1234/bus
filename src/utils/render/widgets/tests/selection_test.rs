use crate::utils::ids::PaneId;
use crate::utils::render::widgets::selection::{
    automatic_selection_bg, automatic_selection_style, relative_luminance,
    render_selection_highlight,
};
use crate::utils::text::selection::Selection;
use crate::utils::theme::Palette;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
#[test]
fn selection_highlight_uses_one_uniform_style() {
    let palette = Palette::catppuccin();
    let host_theme = crate::utils::theme::color::TerminalTheme {
        foreground: None,
        background: Some(crate::utils::theme::color::RgbColor {
            r: 12,
            g: 14,
            b: 16,
        }),
        ..Default::default()
    };
    let expected_style = automatic_selection_style(&palette, host_theme);
    let selection = Some(Selection::absolute_range(
        PaneId::from_raw(1),
        (0, 0),
        (0, 2),
    ));
    let backend = ratatui::backend::TestBackend::new(4, 1);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            let buf = frame.buffer_mut();
            buf[(0, 0)].set_style(
                Style::default()
                    .fg(Color::Rgb(10, 220, 120))
                    .bg(Color::Black),
            );
            buf[(1, 0)].set_style(
                Style::default()
                    .fg(Color::Rgb(220, 180, 40))
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );
            buf[(2, 0)].set_style(Style::default().fg(Color::Blue).bg(Color::Reset));
            render_selection_highlight(
                selection.as_ref(),
                frame.buffer_mut(),
                &PaneId::from_raw(1),
                Rect::new(0, 0, 4, 1),
                None,
                &palette,
                host_theme,
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let first = buffer[(0, 0)].style();
    let second = buffer[(1, 0)].style();
    let third = buffer[(2, 0)].style();

    assert_eq!(first.fg, expected_style.fg);
    assert_eq!(second.fg, expected_style.fg);
    assert_eq!(third.fg, expected_style.fg);
    assert_eq!(first.bg, expected_style.bg);
    assert_eq!(second.bg, expected_style.bg);
    assert_eq!(third.bg, expected_style.bg);
    assert_eq!(first.add_modifier, expected_style.add_modifier);
    assert_eq!(second.add_modifier, expected_style.add_modifier);
    assert_eq!(third.add_modifier, expected_style.add_modifier);
    assert!(!second.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn automatic_selection_background_uses_host_background() {
    let bg = automatic_selection_bg(
        &Palette::terminal(),
        crate::utils::theme::color::TerminalTheme {
            foreground: Some(crate::utils::theme::color::RgbColor {
                r: 230,
                g: 230,
                b: 230,
            }),
            background: Some(crate::utils::theme::color::RgbColor {
                r: 12,
                g: 14,
                b: 16,
            }),
            ..Default::default()
        },
    );

    let Color::Rgb(r, g, b) = bg else {
        panic!("selection background should resolve to rgb");
    };
    assert!(relative_luminance((r, g, b)) > relative_luminance((12, 14, 16)));
}

#[test]
fn automatic_selection_rgb_style_is_readable_with_or_without_host_background() {
    for (background, selected_bg, selected_fg) in [
        ((239, 241, 245), (172, 174, 176), (0, 0, 0)),
        ((26, 27, 38), (90, 91, 99), (255, 255, 255)),
        ((45, 53, 59), (104, 110, 114), (255, 255, 255)),
    ] {
        let mut palette = Palette::catppuccin();
        let (r, g, b) = background;
        palette.panel_bg = Color::Rgb(r, g, b);
        let expected = Style::reset()
            .bg(Color::Rgb(selected_bg.0, selected_bg.1, selected_bg.2))
            .fg(Color::Rgb(selected_fg.0, selected_fg.1, selected_fg.2));

        assert_eq!(
            automatic_selection_style(&palette, Default::default()),
            expected
        );
        assert_eq!(
            automatic_selection_style(
                &Palette::terminal(),
                crate::utils::theme::color::TerminalTheme {
                    background: Some(crate::utils::theme::color::RgbColor { r, g, b }),
                    ..Default::default()
                },
            ),
            expected
        );
    }
}

#[test]
fn automatic_selection_preserves_symbolic_palette_fallbacks() {
    let mut palette = Palette::terminal();
    assert_eq!(
        automatic_selection_style(&palette, Default::default()),
        Style::reset().fg(Color::White).bg(Color::DarkGray)
    );
    for fallback in [Color::Blue, Color::White, Color::Indexed(42), Color::Reset] {
        palette.surface_dim = fallback;
        assert_eq!(
            automatic_selection_bg(&palette, Default::default()),
            fallback
        );
    }
}
