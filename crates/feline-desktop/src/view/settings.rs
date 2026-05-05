use iced::widget::{Space, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Length, Padding};

use crate::app::{Message, SettingsForm, SiteOption};
use crate::theme;
use crate::view::widgets::{
    caption, card, field_label, link_button, page_title, primary_button, section_title, switch,
};

const SITE_OPTIONS: [SiteOption; 2] = [SiteOption::E621, SiteOption::E926];

pub fn view<'a>(form: &'a SettingsForm) -> Element<'a, Message> {
    let header = page_title("Settings", "Configure credentials, download path, and filters.");

    let credentials_card = card(
        column![
            section_title("Credentials"),
            caption(
                "Stored in your OS credential store (Credential Manager · Keychain · Secret Service).",
            ),
            field_label("Username"),
            text_input("", &form.username)
                .on_input(Message::UsernameChanged)
                .style(theme::text_input_style)
                .padding(Padding::from([8u16, 12u16]))
                .size(14),
            field_label("API key"),
            text_input("", &form.api_key)
                .on_input(Message::ApiKeyChanged)
                .secure(true)
                .style(theme::text_input_style)
                .padding(Padding::from([8u16, 12u16]))
                .size(14),
            credentials_status(form),
            credentials_actions(form),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let site_card = card(
        column![
            section_title("Site"),
            field_label("Target"),
            pick_list(
                &SITE_OPTIONS[..],
                Some(form.site),
                Message::SiteChanged,
            )
            .padding(Padding::from([8u16, 12u16])),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let folder_card = card(
        column![
            section_title("Download folder"),
            row![
                text_input("", &form.download_dir)
                    .on_input(Message::DownloadDirChanged)
                    .style(theme::text_input_style)
                    .padding(Padding::from([8u16, 12u16]))
                    .size(14),
                primary_button("Browse…").on_press(Message::PickFolder),
            ]
            .spacing(10),
            caption("Files are saved as {query}/{artist}__{md5}.{ext}"),
        ]
        .spacing(8)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let rating_card = card(
        column![
            section_title("Rating filter"),
            caption("If all or none are enabled, no rating filter is applied."),
            switch("Safe", form.rating_safe, |v| Message::RatingSafe(v)),
            switch("Questionable", form.rating_questionable, |v| {
                Message::RatingQuestionable(v)
            }),
            switch("Explicit", form.rating_explicit, |v| {
                Message::RatingExplicit(v)
            }),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let skip_card = card(
        column![
            section_title("Skip media types"),
            caption("Excluded from every search via -type: filters."),
            switch("Skip videos (.webm)", form.skip_video, |v| {
                Message::SkipVideo(v)
            }),
            switch("Skip flash (.swf)", form.skip_flash, |v| Message::SkipFlash(v)),
            switch("Skip animations (.gif)", form.skip_animation, |v| {
                Message::SkipAnimation(v)
            }),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let blacklist_card = card(
        column![
            section_title("Blacklist tags"),
            caption("One tag per line. Leading '-' is optional — negation is added automatically."),
            text_input("", &form.blacklist)
                .on_input(Message::BlacklistChanged)
                .style(theme::text_input_style)
                .padding(Padding::from([8u16, 12u16]))
                .size(14),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let body = column![
        header,
        credentials_card,
        site_card,
        folder_card,
        rating_card,
        skip_card,
        blacklist_card,
        Space::with_height(Length::Fixed(20.0)),
    ]
    .spacing(16);

    container(
        scrollable(body.width(Length::Fill).padding(Padding::new(32.0)))
            .style(theme::scroller)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .style(theme::page_bg)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn credentials_status<'a>(form: &SettingsForm) -> Element<'a, Message> {
    if !form.creds_error.is_empty() {
        return text(format!("Login failed: {}", form.creds_error))
            .size(12)
            .style(theme::text_danger)
            .into();
    }
    if form.creds_loaded {
        return text("Logged in").size(12).style(theme::text_success).into();
    }
    Space::with_height(Length::Fixed(0.0)).into()
}

fn credentials_actions<'a>(form: &SettingsForm) -> Element<'a, Message> {
    let mut row_widget = row![Space::with_width(Length::Fill)].spacing(8);

    if form.creds_loaded {
        row_widget = row_widget.push(link_button("Log out").on_press(Message::Logout));
    }

    let login_label = if form.creds_checking {
        "Logging in…"
    } else {
        "Log in"
    };
    let mut btn = primary_button(login_label);
    if !form.creds_checking
        && !form.username.trim().is_empty()
        && !form.api_key.trim().is_empty()
    {
        btn = btn.on_press(Message::Login);
    }
    row_widget = row_widget.push(btn);
    row_widget.into()
}
