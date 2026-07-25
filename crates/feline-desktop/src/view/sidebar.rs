use iced::widget::{Space, button, column, container, row, text};
use iced::{Element, Length, Padding};

use crate::app::{Message, Tab};
use crate::theme;

pub fn view<'a>(active_tab: Tab, active_jobs: u32) -> Element<'a, Message> {
    let title = container(text("Feline").size(22).style(theme::text_primary)).padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 24.0,
        left: 8.0,
    });

    let nav = column![
        nav_item("Queue", shortcut("1"), Tab::Queue, active_tab),
        nav_item("Settings", shortcut("2"), Tab::Settings, active_tab),
        nav_item("Log", shortcut("3"), Tab::Log, active_tab),
    ]
    .spacing(4);

    let dot_color = if active_jobs > 0 {
        theme::palette::PRIMARY
    } else {
        theme::palette::TEXT_MUTED
    };
    let status_label = if active_jobs > 0 {
        format!("{active_jobs} active")
    } else {
        "idle".into()
    };
    let status = column![
        row![
            text("●")
                .size(10)
                .style(move |_| iced::widget::text::Style {
                    color: Some(dot_color)
                }),
            text(status_label).size(12).style(theme::text_muted),
        ]
        .spacing(6),
        text("Made with iced").size(10).style(theme::text_muted),
    ]
    .spacing(6)
    .padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 8.0,
    });

    container(
        column![title, nav, Space::new().height(Length::Fill), status]
            .spacing(4)
            .padding(Padding::from([20u16, 12u16])),
    )
    .style(theme::sidebar)
    .width(Length::Fixed(220.0))
    .height(Length::Fill)
    .into()
}

fn nav_item<'a>(label: &str, shortcut: String, tab: Tab, active: Tab) -> Element<'a, Message> {
    let is_active = tab == active;
    button(
        row![
            text(label.to_string()).size(14),
            Space::new().width(Length::Fill),
            text(shortcut).size(11).style(theme::text_muted),
        ]
        .align_y(iced::Alignment::Center),
    )
    .padding(Padding::from([10, 16]))
    .width(Length::Fill)
    .on_press(Message::TabSelected(tab))
    .style(theme::nav_button(is_active))
    .into()
}

#[cfg(target_os = "macos")]
fn shortcut(key: &str) -> String {
    format!("⌘{key}")
}

#[cfg(not(target_os = "macos"))]
fn shortcut(key: &str) -> String {
    format!("Ctrl+{key}")
}
