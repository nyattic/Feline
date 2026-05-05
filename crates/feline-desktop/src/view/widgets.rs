use iced::widget::{Button, button, container, text};
use iced::{Element, Length, Padding};

use crate::theme;

pub fn card<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> container::Container<'a, Message> {
    container(content)
        .padding(Padding::new(0.0))
        .style(theme::card)
}

pub fn page_title<'a, Message: 'a>(title: &str, subtitle: &str) -> Element<'a, Message> {
    let mut col = iced::widget::column![text(title.to_string()).size(26)]
        .spacing(4)
        .width(Length::Fill);
    if !subtitle.is_empty() {
        col = col.push(
            text(subtitle.to_string())
                .size(13)
                .style(theme::text_muted),
        );
    }
    col.into()
}

pub fn section_title<'a, Message: 'a>(label: &str) -> Element<'a, Message> {
    text(label.to_string()).size(15).into()
}

pub fn caption<'a, Message: 'a>(label: &str) -> Element<'a, Message> {
    text(label.to_string()).size(12).style(theme::text_muted).into()
}

pub fn field_label<'a, Message: 'a>(label: &str) -> Element<'a, Message> {
    text(label.to_string()).size(13).into()
}

pub fn primary_button<'a, Message: Clone + 'a>(label: &str) -> Button<'a, Message> {
    button(text(label.to_string()).size(13))
        .padding(Padding::from([10u16, 20u16]))
        .style(theme::primary_button)
}

pub fn link_button<'a, Message: Clone + 'a>(label: &str) -> Button<'a, Message> {
    button(text(label.to_string()).size(13))
        .padding(Padding::from([8u16, 14u16]))
        .style(theme::link_button)
}

pub fn divider<'a, Message: 'a>() -> Element<'a, Message> {
    container(iced::widget::Space::with_height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(theme::divider)
        .into()
}

pub fn switch<'a, Message: Clone + 'a>(
    label: &str,
    checked: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> Element<'a, Message> {
    iced::widget::checkbox(label.to_string(), checked)
        .on_toggle(on_toggle)
        .text_size(13)
        .size(20)
        .into()
}
