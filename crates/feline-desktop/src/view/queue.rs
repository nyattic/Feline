use iced::Color;
use iced::widget::{Space, column, container, progress_bar, row, scrollable, text, text_input};
use iced::{Element, Length, Padding};

use crate::app::{JobView, Message, QueryView};
use crate::theme;
use crate::view::widgets::{card, divider, link_button, page_title, primary_button};

const TAG_DISPLAY_CHARS: usize = 96;
const FILE_DISPLAY_CHARS: usize = 120;

pub fn view<'a>(
    queries: Vec<QueryView>,
    jobs: Vec<JobView>,
    new_query_buf: &'a str,
    logged_in: bool,
) -> Element<'a, Message> {
    let active_count = jobs.iter().filter(|j| !j.finished).count();
    let queued_count = queries.iter().filter(|q| q.queued).count();
    let header = page_title(
        "Queue",
        "Saved tag searches re-run against local MD5s, so existing files are skipped.",
    );

    let summary = row![
        badge(
            format!("{} saved", queries.len()),
            theme::palette::TEXT_MUTED
        ),
        badge(
            format!("{active_count} active"),
            if active_count > 0 {
                theme::palette::PRIMARY
            } else {
                theme::palette::TEXT_MUTED
            }
        ),
        badge(
            format!("{queued_count} queued"),
            if queued_count > 0 {
                theme::palette::WARN
            } else {
                theme::palette::TEXT_MUTED
            }
        ),
    ]
    .spacing(8);

    let mut input_field = text_input("tags (e.g. canine solo -young)", new_query_buf)
        .on_input(Message::NewQueryChanged)
        .style(theme::text_input_style)
        .padding(Padding::from([8u16, 12u16]))
        .size(14);
    if logged_in && !new_query_buf.trim().is_empty() {
        input_field = input_field.on_submit(Message::StartJob(new_query_buf.to_string()));
    }

    let mut input_body = column![
        row![input_field, {
            let btn = primary_button("Download");
            if logged_in && !new_query_buf.trim().is_empty() {
                btn.on_press(Message::StartJob(new_query_buf.to_string()))
            } else {
                btn
            }
        },]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(8)
    .padding(Padding::from([14u16, 20u16]));

    if !logged_in {
        input_body = input_body.push(
            container(
                row![
                    text("Log in from Settings before downloading.")
                        .size(12)
                        .style(theme::text_warn),
                    Space::new().width(Length::Fill),
                    link_button("Open Settings")
                        .on_press(Message::TabSelected(crate::app::Tab::Settings)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .style(theme::subtle_panel)
            .padding(Padding::from([8u16, 12u16])),
        );
    }

    if logged_in && new_query_buf.trim().is_empty() {
        input_body = input_body.push(
            text(
                "Enter one e621/e926 tag search. Filters from Settings are applied automatically.",
            )
            .size(12)
            .style(theme::text_muted),
        );
    }

    let input = card(input_body).width(Length::Fill);

    let body: Element<Message> = if queries.is_empty() {
        empty_state(logged_in)
    } else {
        let mut list = column![].spacing(12);
        for q in queries {
            let matched: Vec<JobView> = jobs.iter().filter(|j| j.tags == q.tags).cloned().collect();
            list = list.push(query_card(q, matched, logged_in));
        }
        scrollable(
            list.padding(Padding::from([0u16, 4u16]))
                .width(Length::Fill),
        )
        .style(theme::scroller)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    };

    container(
        column![header, summary, input, body]
            .spacing(16)
            .padding(Padding::new(32.0)),
    )
    .style(theme::page_bg)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn empty_state<'a>(logged_in: bool) -> Element<'a, Message> {
    let mut body = column![
        text("No saved queries")
            .size(16)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center),
        text(if logged_in {
            "Add a tag search above to create the first reusable download query."
        } else {
            "Log in, then add a tag search to create the first reusable download query."
        })
        .size(13)
        .style(theme::text_muted)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center),
    ]
    .spacing(6)
    .width(Length::Fill)
    .padding(Padding::from([32u16, 0u16]));

    if !logged_in {
        body = body.push(
            row![
                Space::new().width(Length::Fill),
                link_button("Open Settings")
                    .on_press(Message::TabSelected(crate::app::Tab::Settings)),
                Space::new().width(Length::Fill),
            ]
            .padding(Padding {
                top: 8.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            }),
        );
    }

    card(body).width(Length::Fill).into()
}

fn query_card<'a>(
    q: QueryView,
    matched_jobs: Vec<JobView>,
    logged_in: bool,
) -> Element<'a, Message> {
    let id = q.id;
    let tags_owned = q.tags.clone();
    let running = q.running;
    let queued = q.queued;
    let failed_count = q.failed_count;

    let (status_label, status_color) = if running {
        ("running", theme::palette::PRIMARY)
    } else if queued {
        ("queued", theme::palette::WARN)
    } else {
        ("ready", theme::palette::SUCCESS)
    };

    let display_tags = compact_middle(&q.tags, TAG_DISPLAY_CHARS);
    let title = column![
        row![
            text(display_tags).size(16),
            badge(status_label, status_color),
            Space::new().width(Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
        text("Local matches are skipped by MD5 when this query runs.")
            .size(12)
            .style(theme::text_muted),
    ]
    .spacing(4)
    .width(Length::Fill);

    let mut actions = row![].spacing(8).align_y(iced::Alignment::Center);

    if running || queued {
        actions = actions.push(link_button("Cancel").on_press(Message::CancelQuery(id)));
    } else {
        actions = actions.push(link_button("Remove").on_press(Message::RemoveQuery(id)));
    }

    let download_btn = primary_button("Download");
    let download_btn = if !running && !queued && logged_in {
        download_btn.on_press(Message::StartJob(tags_owned))
    } else {
        download_btn
    };
    actions = actions.push(download_btn);

    let mut body = column![
        row![title, actions]
            .spacing(12)
            .align_y(iced::Alignment::Center)
    ]
    .spacing(10);

    if queued {
        body = body.push(
            container(
                text("Waiting for the active job to finish.")
                    .size(12)
                    .style(theme::text_warn),
            )
            .style(theme::subtle_panel)
            .padding(Padding::from([8u16, 12u16])),
        );
    }

    if failed_count > 0 {
        body = body.push(
            row![
                text(format!("{failed_count} permanently skipped"))
                    .size(12)
                    .style(theme::text_muted),
                link_button("Reset skipped").on_press(Message::ClearQueryFailures(id)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
    }

    for job in matched_jobs {
        body = body.push(divider()).push(job_row(job));
    }

    card(body.padding(Padding::from([14u16, 20u16])))
        .width(Length::Fill)
        .into()
}

fn job_row<'a>(job: JobView) -> Element<'a, Message> {
    let phase_color = match job.phase_color_idx {
        0 => theme::palette::TEXT_MUTED,
        2 => theme::palette::SUCCESS,
        3 => theme::palette::WARN,
        4 => theme::palette::DANGER,
        _ => theme::palette::PRIMARY,
    };
    let phase_label = format!("{}{}", job.phase_label, job.phase_dots);
    let phase_text = badge(phase_label, phase_color);

    let stats = text(job.stats_label).size(12).style(theme::text_muted);

    let mut header = row![phase_text, Space::new().width(Length::Fill), stats]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    if !job.finished {
        let label = if job.paused { "Resume" } else { "Pause" };
        header = header.push(link_button(label).on_press(Message::TogglePauseJob(job.id)));
    }

    let bar = progress_bar(0.0..=1.0, job.progress)
        .girth(Length::Fixed(8.0))
        .style(theme::progress_style);

    let mut body = column![header, bar].spacing(6);

    if !job.current_file.is_empty() {
        body = body.push(
            row![
                text("Current").size(11).style(theme::text_muted),
                text(compact_middle(&job.current_file, FILE_DISPLAY_CHARS))
                    .size(11)
                    .style(theme::text_muted),
            ]
            .spacing(8),
        );
    }

    body.into()
}

fn badge<'a>(label: impl Into<String>, color: Color) -> Element<'a, Message> {
    container(text(label.into()).size(11))
        .padding(Padding::from([3u16, 8u16]))
        .style(theme::badge_tinted(color))
        .into()
}

fn compact_middle(value: &str, max_chars: usize) -> String {
    let len = value.chars().count();
    if len <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }

    let head_len = (max_chars - 1) / 2;
    let tail_len = max_chars - 1 - head_len;
    let head: String = value.chars().take(head_len).collect();
    let tail: String = value
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::compact_middle;

    #[test]
    fn compact_middle_keeps_short_values() {
        assert_eq!(compact_middle("abc", 10), "abc");
    }

    #[test]
    fn compact_middle_preserves_front_and_back() {
        assert_eq!(compact_middle("abcdefghijkl", 7), "abc…jkl");
    }
}
