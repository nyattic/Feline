use iced::widget::{Space, column, container, row, scrollable, text};
use iced::{Color, Element, Length, Padding};

use crate::app::{LogLevel, LogLine, Message};
use crate::theme;
use crate::view::widgets::{card, link_button, page_title};

pub fn view<'a>(logs: Vec<LogLine>) -> Element<'a, Message> {
    let warn_count = logs
        .iter()
        .filter(|line| matches!(line.level, LogLevel::Warn))
        .count();
    let error_count = logs
        .iter()
        .filter(|line| matches!(line.level, LogLevel::Error))
        .count();
    let header = row![
        page_title(
            "Log",
            "Recent app events, download failures, and credential errors."
        ),
        Space::new().width(Length::Fill),
        link_button("Clear").on_press(Message::ClearLogs),
    ]
    .align_y(iced::Alignment::Center);

    let summary = row![
        badge(format!("{} lines", logs.len()), theme::palette::TEXT_MUTED),
        badge(
            format!("{warn_count} warnings"),
            if warn_count > 0 {
                theme::palette::WARN
            } else {
                theme::palette::TEXT_MUTED
            }
        ),
        badge(
            format!("{error_count} errors"),
            if error_count > 0 {
                theme::palette::DANGER
            } else {
                theme::palette::TEXT_MUTED
            }
        ),
    ]
    .spacing(8);

    let body: Element<Message> = if logs.is_empty() {
        card(
            column![
                text("No log output")
                    .size(16)
                    .style(theme::text_muted)
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
                text("Warnings and errors from login, search, and downloads appear here.")
                    .size(12)
                    .style(theme::text_muted)
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
            ]
            .spacing(6)
            .width(Length::Fill)
            .padding(Padding::from([24u16, 0u16])),
        )
        .width(Length::Fill)
        .into()
    } else {
        let mut col = column![].spacing(2).width(Length::Fill);
        for line in logs {
            let style: fn(&iced::Theme) -> iced::widget::text::Style = match line.level {
                LogLevel::Info => |_| iced::widget::text::Style { color: None },
                LogLevel::Warn => theme::text_warn,
                LogLevel::Error => theme::text_danger,
            };
            let (label, color) = match line.level {
                LogLevel::Info => ("INFO", theme::palette::TEXT_MUTED),
                LogLevel::Warn => ("WARN", theme::palette::WARN),
                LogLevel::Error => ("ERROR", theme::palette::DANGER),
            };
            col = col.push(
                row![
                    badge(label, color),
                    text(line.text).size(12).style(style).width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }
        card(
            scrollable(col.padding(Padding::from([14u16, 20u16])))
                .style(theme::scroller)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    };

    container(
        column![header, summary, body]
            .spacing(16)
            .padding(Padding::new(32.0)),
    )
    .style(theme::page_bg)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn badge<'a>(label: impl Into<String>, color: Color) -> Element<'a, Message> {
    container(text(label.into()).size(11))
        .padding(Padding::from([3u16, 8u16]))
        .style(theme::badge_tinted(color))
        .into()
}
