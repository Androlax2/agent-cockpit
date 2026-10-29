use super::*;

impl App {
    pub(super) fn build_reviews(&mut self) {
        let (rows, error, loading) = self.reviews.current();
        self.review_rows = rows;
        let mut lines = Vec::new();
        if !error.is_empty() {
            lines.push(Line::new(vec![segment(0, error, style(YELLOW))]));
        } else if self.review_rows.is_empty() && !loading {
            lines.push(Line::new(vec![segment(
                0,
                "No open pull request here.",
                dim(FG_DARK),
            )]));
        }
        for (index, row) in self.review_rows.iter().enumerate() {
            let label = format!("{}#{}", row.repository, row.number);
            let mut right = Vec::new();
            if let Ok(state) = &self.continuity
                && let Some(expiry) = crate::continuity::DeferralTarget::review(row)
                    .and_then(|target| state.deferred_until(&target, util::now_secs()))
            {
                right.push(part(
                    format!(
                        "Later · back in {}",
                        util::duration((expiry - util::now_secs()).max(0))
                    ),
                    style(CYAN),
                ));
            }
            if row.draft {
                right.push(part("draft", dim(FG_DARK)));
            }
            match row.review.as_str() {
                "APPROVED" => right.push(part("approved", style(GREEN))),
                "CHANGES_REQUESTED" => right.push(part("changes", style(RED))),
                "REVIEW_REQUIRED" => right.push(part("review", style(YELLOW))),
                _ => {}
            }
            match row.ci.as_str() {
                "failing" => right.push(part("✗", bold(RED))),
                "pending" => right.push(part("●", bold(YELLOW))),
                "passing" => right.push(part("✓", bold(GREEN))),
                _ => {}
            }
            right.push(part(reviews::review_age(&row.updated), dim(FG_DARK)));
            right.retain(|p| !p.text.is_empty());
            lines.push(Line {
                segments: vec![
                    segment(0, &label, bold(MAGENTA)),
                    segment(1 + width_of(&label) as u16, &row.title, style(FG_DARK)),
                ],
                right,
                item: Some(index),
                accent: None,
                fill: None,
            });
        }
        let panel = &mut self.panels[2];
        panel.items = self.review_rows.len();
        panel.clamp();
        panel.lines = lines;
    }

    /// An unfocused Pull requests panel shows a few rows: its border counts all of them.
    pub(super) fn review_counts(&self) -> Vec<(String, Style)> {
        if self.review_rows.is_empty() {
            return Vec::new();
        }
        let count =
            |state: fn(&Review) -> bool| self.review_rows.iter().filter(|row| state(row)).count();
        let mut counts = vec![(format!("{} open", self.review_rows.len()), bold(FG_DARK))];
        for (number, label, count_style) in [
            (count(|row| row.ci == "failing"), "✗", bold(RED)),
            (
                count(|row| row.review == "CHANGES_REQUESTED"),
                "changes",
                style(RED),
            ),
            (
                count(|row| row.review == "REVIEW_REQUIRED"),
                "review",
                style(YELLOW),
            ),
        ] {
            if number > 0 {
                counts.push((format!("{label} {number}"), count_style));
            }
        }
        counts
    }

    pub(super) fn reviews_subtitle(&self) -> String {
        let scope = self
            .reviews
            .repositories
            .iter()
            .map(|r| r.rsplit('/').next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(", ");
        let scope = if scope.is_empty() {
            "awaiting my review".to_string()
        } else {
            scope
        };
        scope
            + if self.reviews.current().2 {
                "  ⟳"
            } else {
                ""
            }
    }

    pub(crate) fn handle_reviews(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let row = self.review_rows.get(self.panels[2].selected).cloned();
        match key {
            Key::Char('o') => row
                .map(|row| open_in_scratchpad("prs", &row.url))
                .unwrap_or_default(),
            Key::Char('l') => self.toggle_later(),
            Key::Char('t') => self.run_continuity_action("task-trail", screen),
            Key::Enter => match row {
                Some(row) => match self
                    .reviews
                    .checkouts
                    .iter()
                    .find(|(repository, _)| repository.eq_ignore_ascii_case(&row.full_repository))
                    .map(|(_, checkout)| checkout)
                {
                    None => format!(
                        "no local checkout of {}: start a session there first",
                        row.repository
                    ),
                    Some(checkout) => {
                        let agent = match settings::launch_agent() {
                            Ok(agent) => agent,
                            Err(error) => return error,
                        };
                        manager_in_background(&[
                            "--agent",
                            &agent,
                            "review",
                            checkout,
                            &row.number.to_string(),
                        ]);
                        format!("preparing a review of {}#{} …", row.repository, row.number)
                    }
                },
                None => String::new(),
            },
            _ => String::new(),
        }
    }
}
