use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Padding};

use crate::app::{LogLevel, LogLine, Message};
use crate::theme;
use crate::view::widgets::{card, link_button, page_title};

pub fn view<'a>(logs: Vec<LogLine>) -> Element<'a, Message> {
    let header = row![
        page_title("Log", ""),
        link_button("Clear").on_press(Message::ClearLogs),
    ]
    .align_y(iced::Alignment::Center);

    let body: Element<Message> = if logs.is_empty() {
        card(
            column![text("No log output yet.")
                .size(13)
                .style(theme::text_muted)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center)]
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
            col = col.push(text(line.text).size(12).style(style));
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
        column![header, body]
            .spacing(20)
            .padding(Padding::new(32.0)),
    )
    .style(theme::page_bg)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
