use iced::widget::{
    Space, column, container, pick_list, row, scrollable, text, text_editor, text_input,
};
use iced::{Color, Element, Length, Padding};

use crate::app::{DownloadLimitOption, Message, SettingsForm, SiteOption};
use crate::theme;
use crate::view::widgets::{
    caption, card, field_label, link_button, page_title, primary_button, section_title, switch,
};

const SITE_OPTIONS: [SiteOption; 2] = [SiteOption::E621, SiteOption::E926];
const DOWNLOAD_LIMIT_OPTIONS: [DownloadLimitOption; 5] = [
    DownloadLimitOption::Posts500,
    DownloadLimitOption::Posts2000,
    DownloadLimitOption::Posts5000,
    DownloadLimitOption::Posts10000,
    DownloadLimitOption::Unlimited,
];

pub fn view<'a>(
    form: &'a SettingsForm,
    blacklist_content: &'a text_editor::Content,
) -> Element<'a, Message> {
    let header = page_title(
        "Settings",
        "Credentials, destination, and filters apply to every queued download.",
    );

    let status_color = if form.creds_dirty {
        theme::palette::WARN
    } else if form.creds_loaded {
        theme::palette::SUCCESS
    } else {
        theme::palette::WARN
    };
    let status_label = if form.creds_dirty {
        "changes pending"
    } else if form.creds_loaded {
        "credentials saved"
    } else {
        "login required"
    };
    let status_panel = container(
        row![
            badge(status_label, status_color),
            text(if form.creds_dirty && form.creds_loaded {
                "Log in to apply changes. Saved credentials remain active until then."
            } else if form.creds_dirty {
                "Log in to verify and save these credentials."
            } else if form.creds_loaded {
                "Downloads can start from the Queue page."
            } else {
                "Enter your username and API key, then log in once."
            })
            .size(12)
            .style(theme::text_muted),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    )
    .style(theme::subtle_panel)
    .padding(Padding::from([10u16, 14u16]))
    .width(Length::Fill);

    let mut username_input = text_input("", &form.username)
        .style(theme::text_input_style)
        .padding(Padding::from([8u16, 12u16]))
        .size(14);
    if !form.creds_checking {
        username_input = username_input.on_input(Message::UsernameChanged);
    }

    let mut api_key_input = text_input(api_key_placeholder(form), &form.api_key)
        .secure(true)
        .style(theme::text_input_style)
        .padding(Padding::from([8u16, 12u16]))
        .size(14);
    if !form.creds_checking {
        api_key_input = api_key_input.on_input(Message::ApiKeyChanged);
    }

    let credentials_card = card(
        column![
            section_title("Credentials"),
            caption("Saved to the OS credential store after login."),
            field_label("Username"),
            username_input,
            field_label("API key"),
            api_key_input,
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
            caption("Choose the API host used for searches and login checks."),
            pick_list(&SITE_OPTIONS[..], Some(form.site), Message::SiteChanged,)
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
            caption("Saved as {query}/{artist}__{md5}.{ext}. Existing MD5s are skipped."),
        ]
        .spacing(8)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let rating_card = card(
        column![
            section_title("Rating filter"),
            caption(
                "Choose one or two ratings to narrow results. All or none means no rating filter."
            ),
            switch("Safe", form.rating_safe, Message::RatingSafe),
            switch(
                "Questionable",
                form.rating_questionable,
                Message::RatingQuestionable,
            ),
            switch("Explicit", form.rating_explicit, Message::RatingExplicit),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let skip_card = card(
        column![
            section_title("Skip media types"),
            caption("These become negative type filters on every search."),
            switch("Skip videos (.webm)", form.skip_video, Message::SkipVideo),
            switch("Skip flash (.swf)", form.skip_flash, Message::SkipFlash),
            switch(
                "Skip animations (.gif)",
                form.skip_animation,
                Message::SkipAnimation,
            ),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let limit_card = card(
        column![
            section_title("Per-run download limit"),
            caption("Stops discovery after this many new files have been queued."),
            pick_list(
                &DOWNLOAD_LIMIT_OPTIONS[..],
                Some(form.download_limit),
                Message::DownloadLimitChanged,
            )
            .padding(Padding::from([8u16, 12u16])),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let blacklist_card = card(
        column![
            section_title("Blacklist tags"),
            caption(
                "One tag per line. Leading '-' is optional; Feline adds negation automatically."
            ),
            text_editor(blacklist_content)
                .placeholder("young\nanimated\nflash")
                .on_action(Message::BlacklistEdited)
                .style(theme::text_editor_style)
                .padding(Padding::from([8u16, 12u16]))
                .size(14)
                .height(Length::Fixed(120.0)),
        ]
        .spacing(10)
        .padding(Padding::from([18u16, 20u16])),
    )
    .width(Length::Fill);

    let mut body = column![header, status_panel].spacing(16);
    if !form.config_save_error.is_empty() {
        body = body.push(
            container(
                text(format!(
                    "Settings could not be saved: {}",
                    form.config_save_error
                ))
                .size(12)
                .style(theme::text_danger),
            )
            .style(theme::subtle_panel)
            .padding(Padding::from([10u16, 14u16]))
            .width(Length::Fill),
        );
    }
    body = body
        .push(credentials_card)
        .push(site_card)
        .push(folder_card)
        .push(rating_card)
        .push(skip_card)
        .push(limit_card)
        .push(blacklist_card)
        .push(Space::new().height(Length::Fixed(20.0)));

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
        let suffix = if form.creds_loaded {
            " Saved credentials remain active."
        } else {
            ""
        };
        return text(format!("Login failed: {}{suffix}", form.creds_error))
            .size(12)
            .style(theme::text_danger)
            .into();
    }
    if form.creds_dirty {
        return text("Enter the API key and log in to apply these changes.")
            .size(12)
            .style(theme::text_warn)
            .into();
    }
    if form.creds_loaded {
        return row![
            badge("Logged in", theme::palette::SUCCESS),
            text("API key is saved; it is not shown again.")
                .size(12)
                .style(theme::text_muted),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into();
    }
    Space::new().height(Length::Fixed(0.0)).into()
}

fn credentials_actions<'a>(form: &SettingsForm) -> Element<'a, Message> {
    let mut row_widget = row![Space::new().width(Length::Fill)].spacing(8);

    if form.creds_loaded {
        row_widget = row_widget.push(link_button("Log out").on_press(Message::Logout));
    }

    let login_label = if form.creds_checking {
        "Logging in…"
    } else {
        "Log in"
    };
    let mut btn = primary_button(login_label);
    if !form.creds_checking && !form.username.trim().is_empty() && !form.api_key.trim().is_empty() {
        btn = btn.on_press(Message::Login);
    }
    row_widget = row_widget.push(btn);
    row_widget.into()
}

fn api_key_placeholder(form: &SettingsForm) -> &str {
    if form.api_key_saved && form.api_key.is_empty() {
        "Saved in OS credential store"
    } else {
        ""
    }
}

fn badge<'a>(label: impl Into<String>, color: Color) -> Element<'a, Message> {
    container(text(label.into()).size(11))
        .padding(Padding::from([3u16, 8u16]))
        .style(theme::badge_tinted(color))
        .into()
}
