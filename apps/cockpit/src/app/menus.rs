use super::*;

impl App {
    pub(crate) fn open_effort_menu(&mut self, session: Session, anchor_row: Option<u16>) -> String {
        if session.is_external() {
            return "external session: no terminal target; use its own window".into();
        }
        if session.state == "unknown" {
            return "no effort for a session the daemon does not know".into();
        }
        // Under the card's last line, so the card and its effort stay readable above the menu.
        let selected = self.panels[1].selected;
        let anchor = anchor_row.unwrap_or_else(|| {
            self.hit_lines
                .iter()
                .filter(|(_, (panel, item))| *panel == 1 && *item == Some(selected))
                .map(|(row, _)| *row)
                .max()
                .unwrap_or(HEADER_LINES)
        });
        self.menu = Some(EffortMenu::new(session, anchor));
        String::new()
    }

    pub(super) fn choose_effort(&mut self, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.menu.take() else {
            return String::new();
        };
        let level = EFFORT_LEVELS[menu.selected];
        let session = menu.session;
        if level == menu.current {
            return format!("{} already runs on {level}", session.project);
        }
        if session.state == "working"
            && !self.confirm(
                screen,
                &format!("{} is working: restart it on {level}?", session.project),
            )
        {
            return String::new();
        }
        manager_in_background(&["restart", &session.session, level]);
        format!("restarting {} on {level} effort …", session.project)
    }

    /// Keys and clicks while the effort menu is open; anything else closes it.
    pub(super) fn handle_menu(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.menu.as_mut() else {
            return String::new();
        };
        match key {
            Key::Click { x, y, .. } => {
                if let Some(index) = menu.rows.get(&y).copied() {
                    menu.selected = index;
                    return self.choose_effort(screen);
                }
                if !menu.contains(x, y) {
                    self.menu = None;
                }
            }
            Key::Char('j') | Key::Down => {
                menu.selected = (menu.selected + 1).min(EFFORT_LEVELS.len() - 1)
            }
            Key::Char('k') | Key::Up => menu.selected = menu.selected.saturating_sub(1),
            Key::Char(digit @ '1'..='5') => {
                menu.selected = digit as usize - '1' as usize;
                return self.choose_effort(screen);
            }
            Key::Enter => return self.choose_effort(screen),
            _ => self.menu = None,
        }
        String::new()
    }

    pub(super) fn toggle_setting(&mut self, index: usize) -> String {
        let (name, label) = SETTINGS[index];
        let wanted = if settings::is_on(name) { "off" } else { "on" };
        let failure = manager(&["setting", name, wanted]);
        if failure.is_empty() {
            format!("{label}: {wanted}")
        } else {
            failure
        }
    }

    /// What Enter or a click does on a row of the settings menu.
    pub(super) fn choose_settings_row(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.settings_menu.as_ref() else {
            return String::new();
        };
        match menu.row(index) {
            SettingsRow::Setting(setting) => self.toggle_setting(setting),
            SettingsRow::Launch(setting) => self.choose_launch_setting(setting, screen),
            SettingsRow::PairPhone => {
                self.settings_menu = None;
                self.start_pairing()
            }
            SettingsRow::Phone(_) => String::new(),
        }
    }

    pub(super) fn choose_launch_setting(
        &mut self,
        index: usize,
        screen: &mut dyn Screen,
    ) -> String {
        let (name, label) = LAUNCH_SETTINGS[index];
        if name == "default-agent" {
            let Some(agent) = screen.outside(&mut || pick_agent()) else {
                return String::new();
            };
            if agents::find(&agent).is_none() {
                return format!("invalid default-agent: {agent}");
            }
            let error = manager(&["setting", name, &agent]);
            return if error.is_empty() {
                format!("{label}: {agent}")
            } else {
                error
            };
        }
        if name == "idle-sleep-after" {
            let current = sleep::after_minutes();
            let next = SLEEP_AFTER_CHOICES
                .iter()
                .copied()
                .find(|minutes| *minutes > current)
                .unwrap_or(SLEEP_AFTER_CHOICES[0]);
            let error = manager(&["setting", name, &next.to_string()]);
            return if error.is_empty() {
                format!("{label}: {next}")
            } else {
                error
            };
        }
        let selected_model = if name == "opencode-effort" {
            match settings::launch_value("opencode-model") {
                Ok(model) => model,
                Err(error) => return error,
            }
        } else {
            String::new()
        };
        if name == "opencode-effort" && selected_model.is_empty() {
            return "pick an OpenCode model before its effort".into();
        }
        let menu = self
            .settings_menu
            .as_ref()
            .expect("the settings menu is open");
        let Some(catalog) = &menu.catalog else {
            let viewed = current_session();
            let directory = self
                .running_sessions
                .iter()
                .find(|session| session.session == viewed)
                .map(|session| session.cwd.clone())
                .or_else(|| self.selected_session().map(|session| session.cwd))
                .filter(|directory| !directory.is_empty());
            let Some(directory) = directory.or_else(|| {
                screen
                    .outside(&mut || pick_project())
                    .filter(|directory| !directory.is_empty())
            }) else {
                return String::new();
            };
            let catalog = Arc::new(Mutex::new(None));
            let shared = catalog.clone();
            std::thread::spawn(move || {
                let result = settings::load_models(&directory);
                *shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
            });
            self.settings_menu
                .as_mut()
                .expect("the settings menu is open")
                .catalog = Some(catalog);
            return "loading OpenCode models … press Enter again when ready".into();
        };
        let result = catalog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let models = match result {
            None => return "loading OpenCode models … press Enter again when ready".into(),
            Some(Err(error)) => return format!("cannot load OpenCode models: {error}"),
            Some(Ok(models)) => models,
        };
        let mut choices = vec!["OpenCode default".to_string()];
        if name == "opencode-model" {
            choices.extend(models.iter().map(settings::ModelChoice::reference));
        } else {
            let Some(model) = models
                .iter()
                .find(|model| model.reference() == selected_model)
            else {
                return format!("OpenCode model unavailable in this project: {selected_model}");
            };
            choices.extend(model.variants.iter().map(|variant| variant.id.clone()));
        }
        let Some(choice) = screen.outside(&mut || fzf(&format!("{label}> "), &choices, &[])) else {
            return String::new();
        };
        if !choices.contains(&choice) {
            return format!("invalid {label}: {choice}");
        }
        let value = if choice == "OpenCode default" {
            ""
        } else {
            &choice
        };
        let error = manager(&["setting", name, value]);
        if error.is_empty() {
            format!("{label}: {choice}")
        } else {
            error
        }
    }

    /// The QR code of a new one-time pairing link, over the column.
    pub(super) fn start_pairing(&mut self) -> String {
        match remote::pairing_link("") {
            Ok((link, digits)) => {
                self.pairing = Some(Pairing {
                    modules: remote::qr_modules(&link),
                    link,
                    digits,
                    phones_before: remote::phones().len(),
                    paired: None,
                });
                String::new()
            }
            Err(error) => format!("cannot pair a phone: {error}"),
        }
    }

    pub(super) fn forget_phone(&mut self, phone: usize, screen: &mut dyn Screen) -> String {
        let Some(chosen) = self
            .settings_menu
            .as_ref()
            .and_then(|menu| menu.phones.get(phone))
            .cloned()
        else {
            return String::new();
        };
        let name = chosen["name"].as_str().unwrap_or("").to_string();
        if !self.confirm(
            screen,
            &format!("Forget {name}? It needs a new pairing to come back."),
        ) {
            return String::new();
        }
        let outcome = match remote::unpair(chosen["id"].as_str().unwrap_or("")) {
            Ok(name) => format!("{name} forgotten"),
            Err(error) => error,
        };
        if let Some(menu) = self.settings_menu.as_mut() {
            menu.phones = remote::phones();
            menu.selected = menu.selected.min(menu.row_count() - 1);
        }
        outcome
    }

    /// Keys and clicks while the settings menu is open; anything else closes it.
    pub(super) fn handle_settings_menu(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.settings_menu.as_mut() else {
            return String::new();
        };
        match key {
            Key::Click { x, y, .. } => {
                if let Some(index) = menu.rows.get(&y).copied() {
                    menu.selected = index;
                    return self.choose_settings_row(index, screen);
                }
                if !menu.contains(x, y) {
                    self.settings_menu = None;
                }
            }
            Key::Char('j') | Key::Down => {
                menu.selected = (menu.selected + 1).min(menu.row_count() - 1)
            }
            Key::Char('k') | Key::Up => menu.selected = menu.selected.saturating_sub(1),
            Key::Enter | Key::Char(' ') => {
                let index = menu.selected;
                return self.choose_settings_row(index, screen);
            }
            Key::Char('x') => {
                if let SettingsRow::Phone(phone) = menu.row(menu.selected) {
                    return self.forget_phone(phone, screen);
                }
            }
            Key::Char('d') => {
                if let SettingsRow::Launch(setting @ (1 | 2)) = menu.row(menu.selected) {
                    let (name, label) = LAUNCH_SETTINGS[setting];
                    let error = manager(&["setting", name, ""]);
                    return if error.is_empty() {
                        format!("{label}: OpenCode default")
                    } else {
                        error
                    };
                }
            }
            _ => self.settings_menu = None,
        }
        String::new()
    }

    /// Once the phone opened the pairing link, the overlay says so.
    pub(super) fn watch_pairing(&mut self) {
        let Some(pairing) = self
            .pairing
            .as_mut()
            .filter(|pairing| pairing.paired.is_none())
        else {
            return;
        };
        let phones = remote::phones();
        if phones.len() > pairing.phones_before {
            pairing.paired = phones
                .last()
                .and_then(|phone| phone["name"].as_str())
                .map(str::to_string);
        }
    }
}

/// The effort picker over a session card. Choosing another level restarts the session on it.
pub struct EffortMenu {
    pub(crate) session: Session,
    anchor_row: u16,
    pub(crate) current: String,
    pub(crate) selected: usize,
    rows: HashMap<u16, usize>,
    area: (u16, u16, u16, u16), // top, left, bottom, right
}

const MENU_WIDTH: u16 = 30;

impl EffortMenu {
    pub(super) fn new(session: Session, anchor_row: u16) -> Self {
        let current = session.facts.effort.clone();
        let selected = EFFORT_LEVELS
            .iter()
            .position(|level| *level == current)
            .unwrap_or(2);
        Self {
            session,
            anchor_row,
            current,
            selected,
            rows: HashMap::new(),
            area: (0, 0, 0, 0),
        }
    }

    pub(super) fn draw(&mut self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let box_height = EFFORT_LEVELS.len() as u16 + 3;
        let top = if self.anchor_row + 1 + box_height < height.saturating_sub(1) {
            self.anchor_row + 1
        } else {
            self.anchor_row.saturating_sub(box_height).max(1)
        };
        let left = width.saturating_sub(3 + MENU_WIDTH).max(1);
        let right = left + MENU_WIDTH - 1;
        self.area = (top, left, top + box_height - 1, right);
        let frame = on(BLUE, BG_HIGHLIGHT).add_modifier(Modifier::BOLD);
        let ground = on(FG, BG_HIGHLIGHT);
        let inner = (MENU_WIDTH - 2) as usize;
        put(
            buffer,
            top,
            left,
            &format!("╭{}╮", "─".repeat(inner)),
            frame,
        );
        let title: String = format!(" Effort · {} ", self.session.project)
            .chars()
            .take(inner - 2)
            .collect();
        put(buffer, top, left + 2, &title, frame);
        self.rows.clear();
        for (index, level) in EFFORT_LEVELS.iter().enumerate() {
            let row = top + 1 + index as u16;
            let chosen = index == self.selected;
            put(
                buffer,
                row,
                left,
                &format!("│{}│", " ".repeat(inner)),
                frame,
            );
            put(
                buffer,
                row,
                left + 1,
                &" ".repeat(inner),
                if chosen {
                    on(BG_HIGHLIGHT, BLUE)
                } else {
                    ground
                },
            );
            let is_current = *level == self.current;
            let text_style = if chosen {
                on(BG_HIGHLIGHT, BLUE).add_modifier(Modifier::BOLD)
            } else if is_current {
                on(GREEN, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)
            } else {
                ground
            };
            put(
                buffer,
                row,
                left + 2,
                &format!("{} {level}", if is_current { "●" } else { " " }),
                text_style,
            );
            if is_current {
                put(
                    buffer,
                    row,
                    right - 9,
                    "current",
                    if chosen {
                        on(BG_HIGHLIGHT, BLUE)
                    } else {
                        on(FG_DARK, BG_HIGHLIGHT)
                    },
                );
            }
            self.rows.insert(row, index);
        }
        let note_row = top + 1 + EFFORT_LEVELS.len() as u16;
        put(
            buffer,
            note_row,
            left,
            &format!("│{}│", " ".repeat(inner)),
            frame,
        );
        put(
            buffer,
            note_row,
            left + 2,
            "⏎ restarts on it  esc",
            on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM),
        );
        put(
            buffer,
            note_row + 1,
            left,
            &format!("╰{}╯", "─".repeat(inner)),
            frame,
        );
    }

    pub(super) fn contains(&self, x: u16, y: u16) -> bool {
        let (top, left, bottom, right) = self.area;
        (top..=bottom).contains(&y) && (left..=right).contains(&x)
    }
}

/// The settings and new-session defaults in the column's top right corner (S), and below them the phone
/// page (claude-remote): whether its server answers and the tailnet publishes it, a row that pairs
/// a phone, and the paired phones.
pub struct SettingsMenu {
    pub(crate) selected: usize,
    rows: HashMap<u16, usize>,
    area: (u16, u16, u16, u16), // top, left, bottom, right
    pub(crate) phones: Vec<Value>,
    phone_health: Arc<Mutex<Option<PhoneHealth>>>,
    catalog: Option<ModelCatalog>,
    scroll: usize,
}

/// Whether the phone page's server answers, and whether tailscale serve publishes it (None: Tailscale
/// did not say); checked once the menu opens.
type PhoneHealth = (bool, Option<bool>);
type ModelCatalog = Arc<Mutex<Option<Result<Vec<settings::ModelChoice>, String>>>>;

/// What a row of the settings menu does.
#[derive(Debug, PartialEq)]
pub(crate) enum SettingsRow {
    Setting(usize),
    Launch(usize),
    PairPhone,
    Phone(usize),
}

enum SettingsLine {
    Space,
    Section(&'static str),
    Health,
    /// A row's name and value, then its one dim line under it.
    Choice(usize),
    Explain(usize),
}

impl SettingsMenu {
    pub(super) fn new() -> Self {
        let phone_health = Arc::new(Mutex::new(None));
        let shared = phone_health.clone();
        // Half a second at worst, off the key's path: the menu shows "checking" meanwhile.
        std::thread::spawn(move || {
            let health = (remote::server_answers(), remote::is_published());
            *shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(health);
        });
        Self {
            selected: 0,
            rows: HashMap::new(),
            area: (0, 0, 0, 0),
            phones: remote::phones(),
            phone_health,
            catalog: None,
            scroll: 0,
        }
    }

    fn row_count(&self) -> usize {
        SETTINGS.len() + LAUNCH_SETTINGS.len() + 1 + self.phones.len()
    }

    pub(crate) fn row(&self, index: usize) -> SettingsRow {
        let phone_start = LAUNCH_SETTINGS.len() + SETTINGS.len() - 1;
        match index {
            index if index < LAUNCH_SETTINGS.len() => SettingsRow::Launch(index),
            index if index < phone_start => SettingsRow::Setting(index - LAUNCH_SETTINGS.len() + 1),
            index if index == phone_start => SettingsRow::Setting(0),
            index if index == phone_start + 1 => SettingsRow::PairPhone,
            index => SettingsRow::Phone(index - phone_start - 2),
        }
    }

    fn health_line(&self) -> (String, u8) {
        match *self
            .phone_health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            None => ("server … checking".into(), FG_DARK),
            Some((false, _)) => ("server ● down".into(), RED),
            Some((true, Some(false))) => ("server ● up · not on tailnet".into(), YELLOW),
            Some((true, None)) => ("server ● up · tailnet ?".into(), YELLOW),
            Some((true, Some(true))) => ("server ● up · tailnet ● on".into(), GREEN),
        }
    }

    /// What a row says under its name: its explanation, a phone's last visit, the pairing's terms.
    fn explanation(&self, index: usize) -> String {
        match self.row(index) {
            SettingsRow::Setting(setting) => settings::explanation(SETTINGS[setting].0).into(),
            SettingsRow::Launch(setting) => {
                settings::explanation(LAUNCH_SETTINGS[setting].0).into()
            }
            SettingsRow::PairPhone => "QR code, 10 minutes".into(),
            SettingsRow::Phone(phone) => format!(
                "seen {}",
                remote::seen_ago(self.phones[phone].get("last_seen").and_then(Value::as_i64))
            ),
        }
    }

    /// Grouped settings (page 09, frame 2002): uppercase sections, a dim line under each row, values at the right.
    pub(super) fn draw(&mut self, buffer: &mut Buffer) {
        let width = buffer.area.width;
        let top = HEADER_LINES;
        self.rows.clear();
        if width < 16 || buffer.area.height < top + 6 {
            return;
        }
        let mut lines = Vec::new();
        for index in 0..self.row_count() {
            match self.row(index) {
                SettingsRow::Launch(0) => lines.push(SettingsLine::Section("SESSIONS")),
                SettingsRow::Setting(1) => {
                    lines.push(SettingsLine::Space);
                    lines.push(SettingsLine::Section("AUTOMATION"));
                }
                SettingsRow::Setting(0) => {
                    lines.push(SettingsLine::Space);
                    lines.push(SettingsLine::Section("PHONE"));
                    lines.push(SettingsLine::Health);
                }
                _ => {}
            }
            lines.push(SettingsLine::Choice(index));
            lines.push(SettingsLine::Explain(index));
        }
        let box_height = (lines.len() as u16 + 4).min(buffer.area.height - top - 1);
        let body_height = (box_height - 4) as usize;
        let selected_line = lines
            .iter()
            .position(|line| matches!(line, SettingsLine::Choice(index) if *index == self.selected))
            .unwrap_or(0);
        self.scroll = self.scroll.min(lines.len().saturating_sub(body_height));
        if selected_line < self.scroll {
            self.scroll = selected_line;
        }
        if selected_line + 1 >= self.scroll + body_height {
            self.scroll = (selected_line + 2).saturating_sub(body_height);
        }
        let menu_width = width.saturating_sub(4).min(54);
        let left = width.saturating_sub(3 + menu_width).max(1);
        let right = left + menu_width - 1;
        self.area = (top, left, top + box_height - 1, right);
        let frame = on(FG_DARK, BLACK);
        let ground = on(FG, BLACK);
        let heading = on(FG_DARK, BLACK).add_modifier(Modifier::BOLD);
        let inner = menu_width.saturating_sub(2) as usize;
        let blank = |buffer: &mut Buffer, row: u16| {
            put(
                buffer,
                row,
                left,
                &format!("│{}│", " ".repeat(inner)),
                frame,
            )
        };
        put(
            buffer,
            top,
            left,
            &format!("╭{}╮", "─".repeat(inner)),
            frame,
        );
        put(
            buffer,
            top,
            left + 2,
            " Settings ",
            on(FG, BLACK).add_modifier(Modifier::BOLD),
        );
        let mut row = top + 1;
        for line in lines.iter().skip(self.scroll).take(body_height) {
            blank(buffer, row);
            let index = match line {
                SettingsLine::Section(label) => {
                    put(buffer, row, left + 2, label, heading);
                    row += 1;
                    continue;
                }
                SettingsLine::Space => {
                    row += 1;
                    continue;
                }
                SettingsLine::Health => {
                    let (health, color) = self.health_line();
                    put(
                        buffer,
                        row,
                        left + 4,
                        &fit(&health, inner - 4),
                        on(color, BLACK),
                    );
                    row += 1;
                    continue;
                }
                SettingsLine::Choice(index) | SettingsLine::Explain(index) => *index,
            };
            let chosen = index == self.selected;
            let background = if chosen { BG_HIGHLIGHT } else { BLACK };
            put(
                buffer,
                row,
                left + 1,
                &" ".repeat(inner),
                on(FG, background),
            );
            self.rows.insert(row, index);
            if let SettingsLine::Explain(_) = line {
                put(
                    buffer,
                    row,
                    left + 4,
                    &fit(&self.explanation(index), inner.saturating_sub(5)),
                    on(FG_DARK, background),
                );
                row += 1;
                continue;
            }
            let text_style = if chosen {
                on(FG, background).add_modifier(Modifier::BOLD)
            } else {
                ground
            };
            if chosen {
                put(
                    buffer,
                    row,
                    left + 2,
                    "›",
                    on(BLUE, background).add_modifier(Modifier::BOLD),
                );
            }
            let (label, value, value_style) = match self.row(index) {
                SettingsRow::Setting(setting) => {
                    let is_on = settings::is_on(SETTINGS[setting].0);
                    (
                        SETTINGS[setting].1.to_string(),
                        if is_on { " on " } else { " off " }.to_string(),
                        if is_on {
                            on(GREEN, background).add_modifier(Modifier::BOLD)
                        } else {
                            on(if chosen { FG } else { FG_DARK }, background)
                        },
                    )
                }
                SettingsRow::Launch(setting) => {
                    let (name, label) = LAUNCH_SETTINGS[setting];
                    let (value, color) = match settings::launch_value(name) {
                        Ok(value) if value.is_empty() && name == "idle-sleep-after" => {
                            (format!("{} min", sleep::DEFAULT_AFTER_MIN), FG_DARK)
                        }
                        Ok(value) if value.is_empty() => ("OpenCode default".into(), FG_DARK),
                        Ok(value) if name == "idle-sleep-after" => (format!("{value} min"), BLUE),
                        Ok(value) if name == "default-agent" => (
                            agents::find(&value)
                                .map_or(value.clone(), |agent| agent.label.to_string()),
                            BLUE,
                        ),
                        Ok(value) => (value, BLUE),
                        Err(error) => (error, RED),
                    };
                    (
                        label.to_string(),
                        format!(" {value} ▾ "),
                        on(color, background).add_modifier(Modifier::BOLD),
                    )
                }
                SettingsRow::PairPhone => (
                    "Pair a phone".to_string(),
                    " ⏎ ".to_string(),
                    on(FG_DARK, background),
                ),
                SettingsRow::Phone(phone) => (
                    self.phones[phone]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    if chosen {
                        " x forget ".to_string()
                    } else {
                        String::new()
                    },
                    on(BLUE, background).add_modifier(Modifier::BOLD),
                ),
            };
            let value = fit(&value, inner / 2);
            let value_width = value.width();
            put(
                buffer,
                row,
                left + 4,
                &fit(&label, inner.saturating_sub(value_width + 6)),
                text_style,
            );
            put(
                buffer,
                row,
                right.saturating_sub(value_width as u16 + 1),
                &value,
                value_style,
            );
            row += 1;
        }
        while row < top + box_height - 2 {
            blank(buffer, row);
            row += 1;
        }
        blank(buffer, row);
        let mut keys = vec![("↑↓", "move", false), ("⏎", "change", true)];
        match self.row(self.selected) {
            SettingsRow::Launch(1 | 2) => keys.push(("d", "default", false)),
            SettingsRow::Phone(_) => keys.push(("x", "forget", false)),
            _ => {}
        }
        keys.push(("esc", "close", false));
        let mut column = left + 2;
        for (key, what, primary) in keys {
            let cap = format!(" {key} ");
            if column as usize + cap.width() + what.width() + 2 > right as usize {
                break;
            }
            put(
                buffer,
                row,
                column,
                &cap,
                if primary {
                    on(BLACK, YELLOW)
                } else {
                    on(FG, BG_HIGHLIGHT)
                }
                .add_modifier(Modifier::BOLD),
            );
            column += cap.width() as u16 + 1;
            put(buffer, row, column, what, on(FG_DARK, BLACK));
            column += what.width() as u16 + 2;
        }
        put(
            buffer,
            row + 1,
            left,
            &format!("╰{}╯", "─".repeat(inner)),
            frame,
        );
    }

    pub(super) fn contains(&self, x: u16, y: u16) -> bool {
        let (top, left, bottom, right) = self.area;
        (top..=bottom).contains(&y) && (left..=right).contains(&x)
    }
}

/// A phone being paired: the one-time link's QR code over the column, until a key; it says so once
/// the phone opened it.
pub(crate) struct Pairing {
    pub(crate) link: String,
    pub(crate) digits: String,
    modules: Option<Vec<Vec<bool>>>,
    phones_before: usize,
    pub(crate) paired: Option<String>,
}

impl Pairing {
    pub(super) fn draw(&self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let ground = on(FG, BG_HIGHLIGHT);
        for line in HEADER_LINES..height.saturating_sub(1) {
            put(
                buffer,
                line,
                0,
                &" ".repeat(width.saturating_sub(1) as usize),
                ground,
            );
        }
        let mut row = HEADER_LINES + 1;
        if let Some(name) = &self.paired {
            put(
                buffer,
                row,
                2,
                &format!("{name} is paired ✓"),
                on(GREEN, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
            );
            put(
                buffer,
                row + 2,
                2,
                "any key closes",
                on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM),
            );
            return;
        }
        put(
            buffer,
            row,
            2,
            "Pair a phone",
            on(MAGENTA, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
        );
        row += 2;
        // A scanned link opens in Safari, never in the Home Screen app: that app takes the digits.
        put(
            buffer,
            row,
            2,
            "Home Screen app: type",
            on(FG, BG_HIGHLIGHT),
        );
        put(
            buffer,
            row,
            24,
            &format!("{} {}", &self.digits[..3], &self.digits[3..]),
            on(YELLOW, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
        );
        put(
            buffer,
            row,
            33,
            "(2 minutes, 5 tries)",
            on(FG_DARK, BG_HIGHLIGHT),
        );
        row += 1;
        put(
            buffer,
            row,
            2,
            "Browser: scan within 10 minutes, once",
            on(FG, BG_HIGHLIGHT),
        );
        row += 2;
        let modules = self.modules.as_ref().filter(|modules| {
            modules[0].len() as u16 + 4 <= width
                && (modules.len() as u16).div_ceil(2) + row + 4 <= height
        });
        match modules {
            // Two module rows per terminal row (▀: the top one in the foreground), in true black and
            // white whatever the theme, for the phone's camera.
            Some(modules) => {
                let shade = |is_dark: bool| {
                    if is_dark {
                        Color::Rgb(0, 0, 0)
                    } else {
                        Color::Rgb(255, 255, 255)
                    }
                };
                for pair in modules.chunks(2) {
                    for (column, &is_dark) in pair[0].iter().enumerate() {
                        let below = pair.get(1).is_some_and(|next| next[column]);
                        put(
                            buffer,
                            row,
                            2 + column as u16,
                            "▀",
                            Style::default().fg(shade(is_dark)).bg(shade(below)),
                        );
                    }
                    row += 1;
                }
            }
            None => {
                put(
                    buffer,
                    row,
                    2,
                    "No room for the QR code here (or qrencode is missing): open the link on the phone.",
                    on(YELLOW, BG_HIGHLIGHT),
                );
                row += 1;
            }
        }
        row += 1;
        for line in approval::wrap(&self.link, width.saturating_sub(4) as usize) {
            put(buffer, row, 2, &line, on(FG_DARK, BG_HIGHLIGHT));
            row += 1;
        }
        put(
            buffer,
            row + 1,
            2,
            "any key closes (the link keeps working until it is used or expires)",
            on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM),
        );
    }
}
