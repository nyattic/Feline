use iced::widget::{button, container, progress_bar, scrollable, text, text_input};
use iced::{Background, Border, Color, Shadow, Theme, border};

pub mod palette {
    use iced::Color;

    pub const BG: Color = rgb(0x14, 0x12, 0x18);
    pub const SURFACE: Color = rgb(0x1d, 0x1b, 0x20);
    pub const SURFACE_CONTAINER: Color = rgb(0x22, 0x20, 0x27);
    pub const SURFACE_CONTAINER_HIGH: Color = rgb(0x2b, 0x29, 0x30);
    pub const OUTLINE_VARIANT: Color = rgb(0x49, 0x45, 0x4f);
    pub const PRIMARY: Color = rgb(0xa8, 0xc7, 0xfa);
    pub const PRIMARY_CONTAINER: Color = rgb(0x28, 0x43, 0x8e);
    pub const ON_PRIMARY: Color = rgb(0x06, 0x2e, 0x6f);
    pub const TEXT: Color = rgb(0xe6, 0xe0, 0xe9);
    pub const TEXT_MUTED: Color = rgb(0xca, 0xc4, 0xd0);
    pub const SUCCESS: Color = rgb(0x86, 0xdb, 0x98);
    pub const WARN: Color = rgb(0xff, 0xd0, 0x75);
    pub const DANGER: Color = rgb(0xff, 0xb4, 0xab);

    const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }
}

pub fn build() -> Theme {
    Theme::custom(
        "Feline".into(),
        iced::theme::Palette {
            background: palette::BG,
            text: palette::TEXT,
            primary: palette::PRIMARY,
            success: palette::SUCCESS,
            danger: palette::DANGER,
        },
    )
}

pub fn page_bg(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::BG)),
        text_color: Some(palette::TEXT),
        ..Default::default()
    }
}

pub fn sidebar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::SURFACE)),
        text_color: Some(palette::TEXT),
        ..Default::default()
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::SURFACE_CONTAINER)),
        border: border::rounded(16),
        text_color: Some(palette::TEXT),
        ..Default::default()
    }
}

pub fn divider(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::OUTLINE_VARIANT)),
        ..Default::default()
    }
}

pub fn primary_button(_: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(Background::Color(palette::PRIMARY)),
        text_color: palette::ON_PRIMARY,
        border: border::rounded(20),
        shadow: Shadow::default(),
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(mix(
                palette::PRIMARY,
                Color::WHITE,
                0.08,
            ))),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(mix(palette::PRIMARY, Color::BLACK, 0.1))),
            ..base
        },
        button::Status::Disabled => button::Style {
            background: Some(Background::Color(with_alpha(palette::PRIMARY, 0.45))),
            text_color: with_alpha(palette::ON_PRIMARY, 0.7),
            ..base
        },
    }
}

pub fn link_button(_: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: None,
        text_color: palette::PRIMARY,
        border: border::rounded(20),
        shadow: Shadow::default(),
    };
    match status {
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(Background::Color(with_alpha(palette::PRIMARY, 0.10))),
            ..base
        },
        button::Status::Disabled => button::Style {
            text_color: with_alpha(palette::PRIMARY, 0.5),
            ..base
        },
        button::Status::Active => base,
    }
}

pub fn nav_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let bg = if active {
            Some(Background::Color(palette::PRIMARY_CONTAINER))
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => {
                    Some(Background::Color(palette::SURFACE_CONTAINER_HIGH))
                }
                _ => None,
            }
        };
        button::Style {
            background: bg,
            text_color: if active { palette::TEXT } else { palette::TEXT_MUTED },
            border: border::rounded(20),
            shadow: Shadow::default(),
        }
    }
}

pub fn text_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut base = text_input::default(theme, status);
    base.background = Background::Color(palette::SURFACE_CONTAINER_HIGH);
    base.border = Border {
        color: match status {
            text_input::Status::Focused => palette::PRIMARY,
            _ => palette::OUTLINE_VARIANT,
        },
        width: 1.0,
        radius: 8.0.into(),
    };
    base.value = palette::TEXT;
    base.placeholder = palette::TEXT_MUTED;
    base.icon = palette::TEXT_MUTED;
    base.selection = with_alpha(palette::PRIMARY, 0.4);
    base
}

pub fn progress_style(_: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(palette::SURFACE_CONTAINER_HIGH),
        bar: Background::Color(palette::PRIMARY),
        border: border::rounded(4),
    }
}

pub fn scroller(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    scrollable::default(theme, status)
}

pub fn text_muted(_: &Theme) -> text::Style {
    text::Style {
        color: Some(palette::TEXT_MUTED),
    }
}

pub fn text_warn(_: &Theme) -> text::Style {
    text::Style {
        color: Some(palette::WARN),
    }
}

pub fn text_danger(_: &Theme) -> text::Style {
    text::Style {
        color: Some(palette::DANGER),
    }
}

pub fn text_success(_: &Theme) -> text::Style {
    text::Style {
        color: Some(palette::SUCCESS),
    }
}

pub fn text_primary(_: &Theme) -> text::Style {
    text::Style {
        color: Some(palette::PRIMARY),
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r * (1.0 - t) + b.r * t,
        g: a.g * (1.0 - t) + b.g * t,
        b: a.b * (1.0 - t) + b.b * t,
        a: a.a * (1.0 - t) + b.a * t,
    }
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}
