use super::*;

impl App {
    pub(super) fn build_status(&mut self) {
        let selected_action = self.status_actions.get(self.panels[0].selected).copied();
        let previous_selection = self.panels[0].selected;
        self.status_actions = vec![None; self.quota_rows.len()];
        let room = self.width.saturating_sub(4) as usize;
        let labels: Vec<_> = self
            .quota_rows
            .iter()
            .map(|quota| {
                let window = quota
                    .label
                    .split_once(" · ")
                    .map_or("Quota", |(_, window)| window);
                match window {
                    "5h" => "5-hour".into(),
                    "week" => "Weekly".into(),
                    _ => window.replace(" this week", " weekly"),
                }
            })
            .collect();
        let capacities: Vec<_> = self
            .quota_rows
            .iter()
            .map(|quota| match (quota.state.as_str(), quota.remaining) {
                ("Available", Some(remaining)) => format!("{remaining}%"),
                ("Stale", Some(remaining)) => format!("Stale · last {remaining}%"),
                (state, Some(remaining)) => format!("{remaining}% {state}"),
                (state, None) => state.into(),
            })
            .collect();
        let label_width = labels
            .iter()
            .map(|label| width_of(label))
            .max()
            .unwrap_or(0);
        let capacity_width = capacities
            .iter()
            .map(|text| width_of(text))
            .chain([4])
            .max()
            .unwrap_or(4);
        let forecast_width = self
            .quota_rows
            .iter()
            .map(|quota| width_of(quota.forecast.text()))
            .chain([11])
            .max()
            .unwrap_or(11);
        let reset_width = self
            .quota_rows
            .iter()
            .map(|quota| width_of(quota.resets.trim_start_matches("resets in ")))
            .chain([9])
            .max()
            .unwrap_or(9);
        let capacity_column = 2 + label_width + 2;
        let forecast_column = capacity_column + capacity_width + 2;
        let reset_column = forecast_column + forecast_width + 2;
        let full_table = reset_column + reset_width <= room;
        // Every notice, the Sessions tab showing only the first NOTICES_SHOWN.
        let mut lines = Vec::new();
        if !self.notices.is_empty() {
            lines.push(Line::new(vec![segment(0, "NOTICES", bold(FG_DARK))]));
            for notice in &self.notices {
                lines.push(notice_line(notice));
                if !notice.detail.is_empty() {
                    lines.push(Line {
                        segments: vec![segment(2, notice.detail.clone(), dim(FG_DARK))],
                        accent: Some(bold(notice.tint)),
                        ..Line::default()
                    });
                }
            }
            lines.push(Line::default());
        }
        lines.push(Line::new(vec![segment(0, "USAGE", bold(FG_DARK))]));
        if full_table {
            lines.push(Line::new(vec![
                segment(capacity_column as u16, "LEFT", dim(FG_DARK)),
                segment(forecast_column as u16, "RUNS OUT IN", dim(FG_DARK)),
                segment(reset_column as u16, "RESETS IN", dim(FG_DARK)),
            ]));
        } else {
            lines.push(Line::new(vec![segment(
                0,
                "LEFT · remaining capacity",
                dim(FG_DARK),
            )]));
        }
        let mut previous_provider = "";
        for (index, quota) in self.quota_rows.iter().enumerate() {
            let provider = quota
                .label
                .split_once(" · ")
                .map_or(quota.label.as_str(), |(provider, _)| provider);
            if provider != previous_provider {
                if !previous_provider.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::new(vec![segment(0, provider, bold(FG))]));
                previous_provider = provider;
            }
            let tint = bold(match quota.state.as_str() {
                "Available" if quota.remaining.is_some_and(|remaining| remaining <= 50) => YELLOW,
                "Available" => GREEN,
                "Stale" => YELLOW,
                "Low" => RED,
                "Exhausted" | "Blocked" => RED,
                _ => FG_DARK,
            });
            let forecast_tint = match quota.forecast {
                usage::Forecast::Immediate => bold(RED),
                _ if matches!(quota.state.as_str(), "Low" | "Exhausted" | "Blocked") => bold(RED),
                usage::Forecast::Depletion(_) => bold(YELLOW),
                _ => dim(FG_DARK),
            };
            let reset = if quota.resets.is_empty() {
                "—"
            } else {
                quota.resets.trim_start_matches("resets in ")
            };
            let mut row = Line {
                item: Some(index),
                ..Line::default()
            };
            if capacity_column + capacity_width <= room {
                row.segments = vec![
                    segment(2, &labels[index], style(FG)),
                    segment(capacity_column as u16, &capacities[index], tint),
                ];
            } else {
                for label in wrap_detail_text(&labels[index], room.saturating_sub(2).max(2)) {
                    lines.push(Line {
                        segments: vec![segment(2, label, style(FG))],
                        item: Some(index),
                        ..Line::default()
                    });
                }
                row.segments = vec![segment(
                    4,
                    fit(&capacities[index], room.saturating_sub(4)),
                    tint,
                )];
            }
            if full_table {
                row.segments.push(segment(
                    forecast_column as u16,
                    quota.forecast.text(),
                    forecast_tint,
                ));
                row.segments
                    .push(segment(reset_column as u16, reset, dim(FG_DARK)));
            }
            lines.push(row);
            if !full_table {
                let forecast = match &quota.forecast {
                    usage::Forecast::UntilReset => "Lasts until reset".into(),
                    usage::Forecast::Immediate => "May run out now".into(),
                    _ => format!("Runs out in {}", quota.forecast.text()),
                };
                for (text, tint) in [
                    (forecast, forecast_tint),
                    (format!("Resets in {reset}"), dim(FG_DARK)),
                ] {
                    for text in wrap_detail_text(&text, room.saturating_sub(4).max(2)) {
                        lines.push(Line {
                            segments: vec![segment(4, text, tint)],
                            item: Some(index),
                            ..Line::default()
                        });
                    }
                }
            }
        }
        lines.push(Line::default());
        for text in wrap_detail_text(
            "Estimates use window-average pace. — = no estimate.",
            room.max(2),
        ) {
            lines.push(Line::new(vec![segment(0, text, dim(FG_DARK))]));
        }
        let billing = self.costs.billing_totals();
        // All sessions together, never the sum of the projects' times: parallel sessions overlap.
        let together = self.costs.time_together();
        let mut spending = Vec::new();
        let has_priced_replies = billing.recorded_replies > billing.unpriced_replies;
        if billing.recorded_replies > 0 {
            for (label, totals) in [
                ("API credits (est.)", billing.api),
                ("Subscription (API value)", billing.subscription),
                ("Unknown billing (API value)", billing.unknown),
            ] {
                let amounts = if has_priced_replies {
                    [totals.today, totals.week, totals.month].map(money)
                } else {
                    std::array::from_fn(|_| "—".into())
                };
                spending.push((label, amounts, bold(MAGENTA)));
            }
            let total = [0, 1, 2].map(|period| {
                [billing.api, billing.subscription, billing.unknown]
                    .iter()
                    .map(|totals| [totals.today, totals.week, totals.month][period])
                    .sum::<f64>()
            });
            let amounts = if has_priced_replies {
                total.map(money)
            } else {
                std::array::from_fn(|_| "—".into())
            };
            spending.push(("≈ Total at API prices", amounts, bold(MAGENTA)));
        }
        if together.month >= 60.0 {
            spending.push((
                TIME_LABEL,
                [together.today, together.week, together.month].map(Self::duration_text),
                bold(CYAN),
            ));
        }
        let error = self.costs.error();
        let all_time = self.costs.all_time();
        {
            lines.push(Line::default());
            for text in wrap_detail_text(
                "SPENDING · token-based estimates, not your bill",
                room.max(2),
            ) {
                lines.push(Line::new(vec![segment(0, text, bold(FG_DARK))]));
            }
            if self.costs.is_loading() {
                lines.push(Line::new(vec![segment(0, "Loading usage…", dim(FG_DARK))]));
            } else if billing.recorded_replies == 0 && error.is_empty() {
                lines.push(Line::new(vec![segment(
                    0,
                    "No recorded usage",
                    dim(FG_DARK),
                )]));
            }
            if billing.unpriced_replies > 0 {
                let warning = format!(
                    "Incomplete estimate: {} unpriced replies; amounts exclude them.",
                    billing.unpriced_replies
                );
                for text in wrap_detail_text(&warning, room.max(2)) {
                    lines.push(Line::new(vec![segment(0, text, style(YELLOW))]));
                }
            }
            let label_width = spending
                .iter()
                .map(|(label, _, _)| width_of(label))
                .chain([width_of("All-time API value")])
                .max()
                .unwrap_or(0);
            let amount_width = spending
                .iter()
                .flat_map(|(_, amounts, _)| amounts.iter().map(|text| width_of(text)))
                .chain([5])
                .max()
                .unwrap_or(5);
            let columns =
                [0, 1, 2].map(|index| (label_width + 2 + index * (amount_width + 2)) as u16);
            let periods = ["TODAY", "WEEK", "MONTH"];
            if columns[2] as usize + amount_width <= room {
                if !spending.is_empty() {
                    lines.push(Line::new(
                        periods
                            .iter()
                            .enumerate()
                            .map(|(index, period)| {
                                segment(
                                    columns[index] + (amount_width - width_of(period)) as u16,
                                    *period,
                                    dim(FG_DARK),
                                )
                            })
                            .collect(),
                    ));
                }
                for (label, amounts, tint) in spending {
                    let mut segments = vec![segment(0, label, style(FG))];
                    segments.extend(amounts.iter().enumerate().map(|(index, amount)| {
                        segment(
                            columns[index] + (amount_width - width_of(amount)) as u16,
                            amount,
                            tint,
                        )
                    }));
                    lines.push(Line::new(segments));
                }
            } else {
                for (label, amounts, tint) in spending {
                    for text in wrap_detail_text(label, room.max(2)) {
                        lines.push(Line::new(vec![segment(0, text, style(FG))]));
                    }
                    for (period, amount) in periods.iter().zip(amounts) {
                        for text in wrap_detail_text(
                            &format!("{period}  {amount}"),
                            room.saturating_sub(2).max(2),
                        ) {
                            lines.push(Line::new(vec![segment(2, text, tint)]));
                        }
                    }
                }
            }
            if has_priced_replies {
                let amount = format!("≈ {}", money(all_time));
                let label = "All-time API value";
                if label_width + 2 + width_of(&amount) <= room {
                    lines.push(Line::new(vec![
                        segment(0, label, style(FG)),
                        segment(label_width as u16 + 2, amount, bold(MAGENTA)),
                    ]));
                } else {
                    for text in wrap_detail_text(label, room.max(2)) {
                        lines.push(Line::new(vec![segment(0, text, style(FG))]));
                    }
                    for text in wrap_detail_text(&amount, room.saturating_sub(2).max(2)) {
                        lines.push(Line::new(vec![segment(2, text, bold(MAGENTA))]));
                    }
                }
            }
            if billing.recorded_replies > 0 {
                for explanation in [
                    "Subscription = API-price value, not a subscription charge.",
                    "Unknown = billing mode unverified; includes historical records.",
                ] {
                    for text in wrap_detail_text(explanation, room.max(2)) {
                        lines.push(Line::new(vec![segment(0, text, dim(FG_DARK))]));
                    }
                }
            }
            for text in wrap_detail_text(&error, room.max(2)) {
                lines.push(Line::new(vec![segment(0, text, style(YELLOW))]));
            }
        }
        let mut projects: Vec<String> = self
            .running_sessions
            .iter()
            .map(|session| session.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        let project_times: Vec<(String, String)> = projects
            .into_iter()
            .filter_map(|project| {
                Self::time_spent_text(self.costs.time_spent(&project)).map(|time| (project, time))
            })
            .collect();
        if !project_times.is_empty() {
            lines.push(Line::default());
            lines.push(Line::new(vec![segment(
                0,
                "TIME BY PROJECT",
                bold(FG_DARK),
            )]));
            for (project, time) in project_times {
                let project = crate::teams::sidebar::shown(&project);
                let column = width_of(&project) as u16 + 2;
                lines.push(Line::new(vec![
                    segment(0, project.clone(), bold(project_color(&project))),
                    segment(column, time, dim(FG_DARK)),
                ]));
            }
        }
        lines.push(Line::default());
        // One line; everything else about the Chief is on the 5 Chief tab (frame 3803).
        lines.push(Line::new(vec![segment(0, "CHIEF", bold(FG_DARK))]));
        let chief_line = match &self.chief_status {
            Ok(snapshot) => chief_tab::status_line(snapshot),
            Err(error) => format!(
                "Chief unavailable · {}",
                crate::chief::view::one_line(error, 160)
            ),
        };
        lines.push(Line {
            segments: vec![segment(0, chief_line, style(FG))],
            right: vec![
                part(" 5 ", on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)),
                part("Chief tab", dim(FG_DARK)),
            ],
            ..Line::default()
        });
        lines.push(Line::default());
        lines.push(Line::new(vec![segment(0, "SERVICES", bold(FG_DARK))]));
        for (provider, action) in [
            (&self.claude_status, StatusAction::ClaudeStatus),
            (&self.codex_status, StatusAction::CodexStatus),
        ] {
            let index = self.status_actions.len();
            self.status_actions.push(Some(action));
            lines.extend(
                provider_status_lines(provider, room)
                    .into_iter()
                    .map(|mut line| {
                        line.item = Some(index);
                        line
                    }),
            );
        }
        lines.push(Line::default());
        lines.push(Line::new(vec![segment(0, "UTILITIES", bold(FG_DARK))]));
        for (label, action) in [
            ("Daily digest", StatusAction::Digest),
            ("Clones review / cleanup", StatusAction::Clones),
        ] {
            let index = self.status_actions.len();
            self.status_actions.push(Some(action));
            lines.push(Line {
                segments: vec![segment(0, label, style(FG))],
                item: Some(index),
                ..Default::default()
            });
        }
        let panel = &mut self.panels[0];
        panel.items = self.status_actions.len();
        if let Some(Some(action)) = selected_action {
            panel.selected = self
                .status_actions
                .iter()
                .position(|candidate| *candidate == Some(action))
                .expect("all Status actions remain available");
        } else if selected_action == Some(None) && previous_selection >= self.quota_rows.len() {
            self.status_selection_lost = true;
        }
        panel.clamp();
        panel.lines = lines;
    }

    pub(super) fn status_subtitle(&self) -> String {
        self.plan.clone()
    }
}
