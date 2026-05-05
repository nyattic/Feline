use iced::widget::{Space, column, container, progress_bar, row, scrollable, text, text_input};
use iced::{Element, Length, Padding};

use crate::app::{JobView, Message, QueryView};
use crate::theme;
use crate::view::widgets::{card, divider, link_button, page_title, primary_button};

pub fn view<'a>(
    queries: Vec<QueryView>,
    jobs: Vec<JobView>,
    new_query_buf: &'a str,
    logged_in: bool,
) -> Element<'a, Message> {
    let header = page_title(
        "Queue",
        "Type a tag search and download — saved queries can be re-run for new posts only.",
    );

    let input = card(
        row![
            text_input("tags (e.g. canine solo -young)", new_query_buf)
                .on_input(Message::NewQueryChanged)
                .on_submit(Message::StartJob(new_query_buf.to_string()))
                .style(theme::text_input_style)
                .padding(Padding::from([8u16, 12u16]))
                .size(14),
            {
                let btn = primary_button("Download");
                if logged_in && !new_query_buf.trim().is_empty() {
                    btn.on_press(Message::StartJob(new_query_buf.to_string()))
                } else {
                    btn
                }
            },
        ]
        .spacing(10)
        .padding(Padding::from([14u16, 20u16]))
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill);

    let body: Element<Message> = if queries.is_empty() {
        empty_state()
    } else {
        let mut list = column![].spacing(12);
        for q in queries {
            let matched: Vec<JobView> = jobs.iter().filter(|j| j.tags == q.tags).cloned().collect();
            list = list.push(query_card(q, matched, logged_in));
        }
        scrollable(list.padding(Padding::from([0u16, 4u16])).width(Length::Fill))
            .style(theme::scroller)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    container(
        column![header, input, body]
            .spacing(24)
            .padding(Padding::new(32.0)),
    )
    .style(theme::page_bg)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn empty_state<'a>() -> Element<'a, Message> {
    card(
        column![
            text("No queries yet")
                .size(16)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            text("Add a tag search above to get started.")
                .size(13)
                .style(theme::text_muted)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
        ]
        .spacing(6)
        .width(Length::Fill)
        .padding(Padding::from([32u16, 0u16])),
    )
    .width(Length::Fill)
    .into()
}

fn query_card<'a>(q: QueryView, matched_jobs: Vec<JobView>, logged_in: bool) -> Element<'a, Message> {
    let id = q.id;
    let tags_owned = q.tags.clone();
    let running = q.running;
    let queued = q.queued;
    let failed_count = q.failed_count;

    let mut header = row![text(q.tags).size(16).width(Length::Fill)]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    if running || queued {
        header = header.push(link_button("Cancel").on_press(Message::CancelQuery(id)));
    } else {
        header = header.push(link_button("Remove").on_press(Message::RemoveQuery(id)));
    }

    let download_btn = primary_button("Download");
    let download_btn = if !running && !queued && logged_in {
        download_btn.on_press(Message::StartJob(tags_owned))
    } else {
        download_btn
    };
    header = header.push(download_btn);

    let mut body = column![header].spacing(0);

    if queued {
        body = body
            .push(Space::with_height(Length::Fixed(4.0)))
            .push(
                text("queued — waiting for the active job")
                    .size(12)
                    .style(theme::text_warn),
            );
    }

    if failed_count > 0 {
        body = body
            .push(Space::with_height(Length::Fixed(4.0)))
            .push(
                text(format!("{failed_count} permanently skipped"))
                    .size(12)
                    .style(theme::text_muted),
            );
    }

    for job in matched_jobs {
        body = body
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(divider())
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(job_row(job));
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
    let phase_text = text(phase_label).size(12).style(move |_| {
        iced::widget::text::Style {
            color: Some(phase_color),
        }
    });

    let stats = text(job.stats_label).size(12).style(theme::text_muted);

    let mut header = row![phase_text, Space::with_width(Length::Fill), stats]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    if !job.finished {
        let label = if job.paused { "Resume" } else { "Pause" };
        header = header.push(link_button(label).on_press(Message::TogglePauseJob(job.id)));
    }

    let bar = progress_bar(0.0..=1.0, job.progress)
        .height(Length::Fixed(8.0))
        .style(theme::progress_style);

    let mut body = column![header, bar].spacing(6);

    if !job.current_file.is_empty() {
        body = body.push(text(job.current_file).size(11).style(theme::text_muted));
    }

    body.into()
}
