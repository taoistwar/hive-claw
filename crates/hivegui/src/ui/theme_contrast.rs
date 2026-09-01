use std::rc::Rc;

use gpui::{App, Hsla};
use gpui_component::theme::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};

const TEXT_CONTRAST: f32 = 4.52;
const GRAPHIC_CONTRAST: f32 = 3.02;

/// Installs HiveGUI's WCAG contrast policy and applies it to the active theme.
pub(crate) fn install(cx: &mut App) {
    apply_to_active_theme(cx);
    cx.observe_global::<ThemeRegistry>(apply_to_active_theme)
        .detach();
}

/// Applies a registry theme while preserving its palette wherever it already
/// meets HiveGUI's contrast contract.
pub(crate) fn apply_config(config: &Rc<ThemeConfig>, cx: &mut App) {
    let config = accessible_config(config);
    let mode = config.mode;
    Theme::global_mut(cx).apply_config(&config);
    // `Theme::change` republishes the adjusted semantic tokens to gpui-base,
    // which owns the real focus-ring rendering used by gpui-component.
    Theme::change(mode, None, cx);
}

fn apply_to_active_theme(cx: &mut App) {
    let config = {
        let theme = Theme::global(cx);
        if theme.mode.is_dark() {
            theme.dark_theme.clone()
        } else {
            theme.light_theme.clone()
        }
    };
    apply_config(&config, cx);
}

fn accessible_config(config: &Rc<ThemeConfig>) -> Rc<ThemeConfig> {
    let defaults = if config.mode == ThemeMode::Dark {
        gpui_component::theme::ThemeColor::dark()
    } else {
        gpui_component::theme::ThemeColor::light()
    };
    let mut theme = Theme::from(defaults.as_ref());
    theme.apply_config(config);
    harden_theme(&mut theme);

    let mut config = (**config).clone();
    macro_rules! persist {
        ($($field:ident),+ $(,)?) => {
            $(config.colors.$field = Some(color_string(theme.$field));)+
        };
    }
    persist!(
        foreground,
        muted_foreground,
        popover_foreground,
        primary_foreground,
        secondary_foreground,
        accent_foreground,
        group_box_foreground,
        description_list_label_foreground,
        sidebar_foreground,
        sidebar_accent_foreground,
        sidebar_primary_foreground,
        tab_foreground,
        tab_active_foreground,
        table_head_foreground,
        table_foot_foreground,
        danger,
        danger_hover,
        danger_active,
        danger_foreground,
        success,
        success_hover,
        success_active,
        success_foreground,
        warning,
        warning_hover,
        warning_active,
        warning_foreground,
        info,
        info_hover,
        info_active,
        info_foreground,
        button_foreground,
        button_primary_foreground,
        button_secondary_foreground,
        button_danger,
        button_danger_hover,
        button_danger_active,
        button_danger_foreground,
        button_success,
        button_success_hover,
        button_success_active,
        button_success_foreground,
        button_warning,
        button_warning_hover,
        button_warning_active,
        button_warning_foreground,
        button_info,
        button_info_hover,
        button_info_active,
        button_info_foreground,
        ring,
        list_active_border,
        table_active_border,
    );

    Rc::new(config)
}

fn harden_theme(theme: &mut Theme) {
    let canvas = theme.background;
    theme.foreground = contrasting_color(
        theme.foreground,
        &[theme.background, theme.title_bar, theme.status_bar],
        canvas,
        TEXT_CONTRAST,
    );
    theme.muted_foreground = contrasting_color(
        theme.muted_foreground,
        &[theme.background, theme.muted],
        canvas,
        TEXT_CONTRAST,
    );
    theme.popover_foreground = contrasting_color(
        theme.popover_foreground,
        &[theme.popover],
        canvas,
        TEXT_CONTRAST,
    );
    theme.primary_foreground = contrasting_color(
        theme.primary_foreground,
        &[theme.primary, theme.primary_hover, theme.primary_active],
        canvas,
        TEXT_CONTRAST,
    );
    theme.secondary_foreground = contrasting_color(
        theme.secondary_foreground,
        &[
            theme.secondary,
            theme.secondary_hover,
            theme.secondary_active,
        ],
        canvas,
        TEXT_CONTRAST,
    );
    theme.accent_foreground = contrasting_color(
        theme.accent_foreground,
        &[theme.accent],
        canvas,
        TEXT_CONTRAST,
    );
    theme.group_box_foreground = contrasting_color(
        theme.group_box_foreground,
        &[theme.group_box],
        canvas,
        TEXT_CONTRAST,
    );
    theme.description_list_label_foreground = contrasting_color(
        theme.description_list_label_foreground,
        &[theme.description_list_label],
        canvas,
        TEXT_CONTRAST,
    );
    theme.sidebar_foreground = contrasting_color(
        theme.sidebar_foreground,
        &[theme.sidebar],
        canvas,
        TEXT_CONTRAST,
    );
    theme.sidebar_accent_foreground = contrasting_color(
        theme.sidebar_accent_foreground,
        &[theme.sidebar_accent],
        canvas,
        TEXT_CONTRAST,
    );
    theme.sidebar_primary_foreground = contrasting_color(
        theme.sidebar_primary_foreground,
        &[theme.sidebar_primary],
        canvas,
        TEXT_CONTRAST,
    );
    theme.tab_foreground =
        contrasting_color(theme.tab_foreground, &[theme.tab], canvas, TEXT_CONTRAST);
    theme.tab_active_foreground = contrasting_color(
        theme.tab_active_foreground,
        &[theme.tab_active],
        canvas,
        TEXT_CONTRAST,
    );
    theme.table_head_foreground = contrasting_color(
        theme.table_head_foreground,
        &[theme.table_head],
        canvas,
        TEXT_CONTRAST,
    );
    theme.table_foot_foreground = contrasting_color(
        theme.table_foot_foreground,
        &[theme.table_foot],
        canvas,
        TEXT_CONTRAST,
    );

    (
        theme.danger,
        theme.danger_hover,
        theme.danger_active,
        theme.danger_foreground,
    ) = status_colors(
        theme.danger,
        theme.danger_hover,
        theme.danger_active,
        theme.danger_foreground,
        canvas,
    );
    (
        theme.success,
        theme.success_hover,
        theme.success_active,
        theme.success_foreground,
    ) = status_colors(
        theme.success,
        theme.success_hover,
        theme.success_active,
        theme.success_foreground,
        canvas,
    );
    (
        theme.warning,
        theme.warning_hover,
        theme.warning_active,
        theme.warning_foreground,
    ) = status_colors(
        theme.warning,
        theme.warning_hover,
        theme.warning_active,
        theme.warning_foreground,
        canvas,
    );
    (
        theme.info,
        theme.info_hover,
        theme.info_active,
        theme.info_foreground,
    ) = status_colors(
        theme.info,
        theme.info_hover,
        theme.info_active,
        theme.info_foreground,
        canvas,
    );

    theme.button_foreground = contrasting_color(
        theme.button_foreground,
        &[theme.button, theme.button_hover, theme.button_active],
        canvas,
        TEXT_CONTRAST,
    );
    theme.button_primary_foreground = contrasting_color(
        theme.button_primary_foreground,
        &[
            theme.button_primary,
            theme.button_primary_hover,
            theme.button_primary_active,
        ],
        canvas,
        TEXT_CONTRAST,
    );
    theme.button_secondary_foreground = contrasting_color(
        theme.button_secondary_foreground,
        &[
            theme.button_secondary,
            theme.button_secondary_hover,
            theme.button_secondary_active,
        ],
        canvas,
        TEXT_CONTRAST,
    );
    (
        theme.button_danger,
        theme.button_danger_hover,
        theme.button_danger_active,
        theme.button_danger_foreground,
    ) = status_colors(
        theme.button_danger,
        theme.button_danger_hover,
        theme.button_danger_active,
        theme.button_danger_foreground,
        canvas,
    );
    (
        theme.button_success,
        theme.button_success_hover,
        theme.button_success_active,
        theme.button_success_foreground,
    ) = status_colors(
        theme.button_success,
        theme.button_success_hover,
        theme.button_success_active,
        theme.button_success_foreground,
        canvas,
    );
    (
        theme.button_warning,
        theme.button_warning_hover,
        theme.button_warning_active,
        theme.button_warning_foreground,
    ) = status_colors(
        theme.button_warning,
        theme.button_warning_hover,
        theme.button_warning_active,
        theme.button_warning_foreground,
        canvas,
    );
    (
        theme.button_info,
        theme.button_info_hover,
        theme.button_info_active,
        theme.button_info_foreground,
    ) = status_colors(
        theme.button_info,
        theme.button_info_hover,
        theme.button_info_active,
        theme.button_info_foreground,
        canvas,
    );

    theme.ring = contrasting_color(
        theme.ring,
        &[theme.background, theme.popover, theme.sidebar],
        canvas,
        GRAPHIC_CONTRAST,
    );
    theme.list_active_border = contrasting_color(
        theme.list_active_border,
        &[theme.colors.list],
        canvas,
        GRAPHIC_CONTRAST,
    );
    theme.table_active_border = contrasting_color(
        theme.table_active_border,
        &[theme.table],
        canvas,
        GRAPHIC_CONTRAST,
    );
}

fn status_colors(
    base: Hsla,
    hover: Hsla,
    active: Hsla,
    foreground: Hsla,
    canvas: Hsla,
) -> (Hsla, Hsla, Hsla, Hsla) {
    let base = contrasting_color(base, &[canvas], canvas, TEXT_CONTRAST);
    let hover = contrasting_color(hover, &[canvas], canvas, TEXT_CONTRAST);
    let active = contrasting_color(active, &[canvas], canvas, TEXT_CONTRAST);
    let foreground = contrasting_color(foreground, &[base, hover, active], canvas, TEXT_CONTRAST);
    (base, hover, active, foreground)
}

fn contrasting_color(original: Hsla, backgrounds: &[Hsla], canvas: Hsla, minimum: f32) -> Hsla {
    if minimum_contrast(original, backgrounds, canvas) >= minimum {
        return original;
    }

    for step in 1..=1000 {
        let delta = step as f32 / 1000.0;
        let mut best = None;
        for lightness in [original.l - delta, original.l + delta] {
            if !(0.0..=1.0).contains(&lightness) {
                continue;
            }
            let candidate = Hsla {
                l: lightness,
                a: 1.0,
                ..original
            };
            let contrast = minimum_contrast(candidate, backgrounds, canvas);
            if contrast >= minimum && best.is_none_or(|(_, best_contrast)| contrast > best_contrast)
            {
                best = Some((candidate, contrast));
            }
        }
        if let Some((candidate, _)) = best {
            return candidate;
        }
    }

    let black = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: 1.0,
    };
    let white = Hsla { l: 1.0, ..black };
    if minimum_contrast(black, backgrounds, canvas) >= minimum_contrast(white, backgrounds, canvas)
    {
        black
    } else {
        white
    }
}

fn minimum_contrast(foreground: Hsla, backgrounds: &[Hsla], canvas: Hsla) -> f32 {
    backgrounds
        .iter()
        .map(|background| contrast_ratio(foreground, *background, canvas))
        .fold(f32::INFINITY, f32::min)
}

fn contrast_ratio(foreground: Hsla, background: Hsla, canvas: Hsla) -> f32 {
    let canvas = canvas.to_rgb();
    let background = canvas.blend(background.to_rgb());
    let foreground = background.blend(foreground.to_rgb());
    let foreground = relative_luminance([foreground.r, foreground.g, foreground.b]);
    let background = relative_luminance([background.r, background.g, background.b]);
    let (lighter, darker) = if foreground >= background {
        (foreground, background)
    } else {
        (background, foreground)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(color: [f32; 3]) -> f32 {
    let [red, green, blue] = color.map(|channel| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}

fn color_string(color: Hsla) -> gpui::SharedString {
    let color = color.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        channel_byte(color.r),
        channel_byte(color.g),
        channel_byte(color.b),
        channel_byte(color.a),
    )
    .into()
}

fn channel_byte(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}
