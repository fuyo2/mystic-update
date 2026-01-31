// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::fl;
use crate::update::{self, ManagerId, TaskStatus, UpdateEntry, UpdateOutcome};
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::{Alignment, Background, Color, Length, Subscription};
use cosmic::widget::{self, about::About, menu};
use cosmic::{prelude::*, Task};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::Duration;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] =
    include_bytes!("../resources/icons/hicolor/scalable/apps/mystic-update.svg");
const RUNNING_PROGRESS_MIN: f32 = 15.0;
const RUNNING_PROGRESS_MAX: f32 = 85.0;
const RUNNING_PROGRESS_STEP: f32 = 3.0;
const RUNNING_PROGRESS_TICK_MS: u64 = 120;

#[derive(Clone, Debug)]
struct ManagerState {
    available: bool,
    requires_privilege: bool,
    status: TaskStatus,
    last_output: String,
    check_output: String,
    checking: bool,
    update_available: bool,
    progress: Option<f32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct UpdateKey {
    manager: ManagerId,
    id: String,
}

#[derive(Clone, Debug)]
struct UpdateItem {
    entry: UpdateEntry,
    checked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DetailsTab {
    Information,
    Packages,
    Changelog,
}

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// Display a context drawer with the designated page if defined.
    context_page: ContextPage,
    /// The about page for this app.
    about: About,
    /// Key bindings for the application's menu bar.
    key_binds: HashMap<menu::KeyBind, MenuAction>,
    /// Configuration data that persists between application runs.
    config: Config,
    /// Manager status by ID.
    managers: BTreeMap<ManagerId, ManagerState>,
    /// Selected manager for log display.
    selected_manager: Option<ManagerId>,
    /// Cached output to display in the log pane.
    log_text: String,
    /// Queue for sequential "Run All" updates.
    run_all_queue: VecDeque<ManagerId>,
    /// True while a "Run All" sequence is active.
    run_all_active: bool,
    /// Combined output for "Run All".
    run_all_output: String,
    /// Prompt for reboot when apt applies updates.
    reboot_prompt: bool,
    /// Track an in-progress reboot request.
    reboot_in_progress: bool,
    /// Parsed update items for supported managers.
    update_items: Vec<UpdateItem>,
    /// Selected update for the details panel.
    selected_update: Option<UpdateKey>,
    /// Selected tab for update details.
    details_tab: DetailsTab,
    /// True when showing the update details view.
    show_update_details: bool,
    /// Track update selections at the start of a manager run.
    manager_run_selection: HashMap<ManagerId, Option<Vec<UpdateKey>>>,
    /// Animated progress phase for indeterminate update bars.
    progress_phase: f32,
    /// Shared progress tracker for live update operations.
    progress_tracker: update::ProgressTracker,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    LaunchUrl(String),
    ToggleContextPage(ContextPage),
    UpdateConfig(Config),
    DetectComplete(Vec<(ManagerId, bool)>),
    CheckComplete(ManagerId, update::CheckOutcome),
    CheckUpdates,
    RunManager(ManagerId),
    RunAll,
    UpdateComplete(ManagerId, UpdateOutcome),
    ProgressTick,
    SelectManager(ManagerId),
    ToggleUpdate(UpdateKey, bool),
    SelectUpdate(UpdateKey),
    SelectDetailsTab(DetailsTab),
    ShowUpdatesList,
    SelectAllUpdates,
    SelectNoUpdates,
    RebootNow,
    RebootDismiss,
    RebootFinished(Result<(), String>),
}

/// Create a COSMIC application from the app model
impl cosmic::Application for AppModel {
    /// The async executor that will be used to run your application's commands.
    type Executor = cosmic::executor::Default;

    /// Data that your application receives to its init method.
    type Flags = ();

    /// Messages which the application and its widgets will emit.
    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    const APP_ID: &'static str = "dev.jcole.MysticUpdate";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Create the about widget
        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_svg_bytes(APP_ICON))
            .version(env!("CARGO_PKG_VERSION"))
            .links([(fl!("repository"), REPOSITORY)])
            .developers([("Justin Cole", "fuyo2@live.com")])
            .license(env!("CARGO_PKG_LICENSE"));

        let mut managers = BTreeMap::new();
        for id in ManagerId::all() {
            let spec = id.spec();
            managers.insert(
                *id,
                ManagerState {
                    available: false,
                    requires_privilege: spec.requires_privilege,
                    status: TaskStatus::Idle,
                    last_output: String::new(),
                    check_output: String::new(),
                    checking: false,
                    update_available: false,
                    progress: None,
                },
            );
        }

        // Construct the app model with the runtime's core.
        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            about,
            key_binds: HashMap::new(),
            // Optional configuration file for an application.
            config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
                .map(|context| match Config::get_entry(&context) {
                    Ok(config) => config,
                    Err((_errors, config)) => config,
                })
                .unwrap_or_default(),
            managers,
            selected_manager: None,
            log_text: fl!("select-manager"),
            run_all_queue: VecDeque::new(),
            run_all_active: false,
            run_all_output: String::new(),
            reboot_prompt: false,
            reboot_in_progress: false,
            update_items: Vec::new(),
            selected_update: None,
            details_tab: DetailsTab::Information,
            show_update_details: false,
            manager_run_selection: HashMap::new(),
            progress_phase: 0.0,
            progress_tracker: update::new_progress_tracker(),
        };

        let set_title = app.update_title();
        let detect = Task::perform(update::detect_managers(), Message::DetectComplete)
            .map(cosmic::Action::App);

        (app, Task::batch(vec![set_title, detect]))
    }

    /// Elements to pack at the start of the header bar.
    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            menu::root(fl!("view")).apply(Element::from),
            menu::items(
                &self.key_binds,
                vec![
                    menu::Item::Button(fl!("about"), None, MenuAction::About),
                    menu::Item::Button(fl!("summoners"), None, MenuAction::Summoners),
                    menu::Item::Button(fl!("logs"), None, MenuAction::Logs),
                ],
            ),
        )]);

        vec![menu_bar.into()]
    }

    /// Display a context drawer if the context page is requested.
    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
            ContextPage::Summoners => {
                let space_s = cosmic::theme::spacing().space_s;
                let content = widget::container(self.summoners_drawer_content())
                    .width(Length::Fill)
                    .padding(space_s);

                context_drawer::context_drawer(
                    content,
                    Message::ToggleContextPage(ContextPage::Summoners),
                )
                .title(fl!("summoners-title"))
            }
            ContextPage::Logs => {
                let content = widget::scrollable(
                    widget::container(widget::text::body(self.log_text.clone()))
                        .width(Length::Fill)
                        .padding(cosmic::theme::spacing().space_s),
                )
                .height(Length::Fill);

                context_drawer::context_drawer(content, Message::ToggleContextPage(ContextPage::Logs))
                    .title(fl!("logs-title"))
            }
        })
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// Application events will be processed through the view. Any messages emitted by
    /// events received by widgets will be passed to the update method.
    fn view(&self) -> Element<'_, Self::Message> {
        let space_s = cosmic::theme::spacing().space_s;
        let (available_count, _total_count) = self.manager_counts();
        let any_running = self.any_running();
        let any_checking = self.any_checking();
        let any_updates = self.any_updates_available();
        let can_run_all = available_count > 0 && any_updates && !any_running && !any_checking;
        let can_check_updates = available_count > 0 && !any_running && !any_checking;

        let check_updates_button = widget::button::standard(fl!("check-updates"));
        let check_updates_button = if can_check_updates {
            check_updates_button.on_press(Message::CheckUpdates)
        } else {
            check_updates_button
        };

        let run_all_button = widget::button::suggested(fl!("run-all"));
        let run_all_button = if can_run_all {
            run_all_button.on_press(Message::RunAll)
        } else {
            run_all_button
        };

        let header_row: Element<_> = if self.show_update_details {
            widget::row::with_capacity(4)
                .push(self.back_button())
                .push(widget::horizontal_space())
                .push(check_updates_button)
                .push(run_all_button)
                .align_y(Alignment::Center)
                .spacing(space_s)
                .into()
        } else {
            widget::row::with_capacity(4)
                .push(widget::text::title1(fl!("app-title")))
                .push(widget::horizontal_space())
                .push(check_updates_button)
                .push(run_all_button)
                .align_y(Alignment::Center)
                .spacing(space_s)
                .into()
        };
        let header = widget::container(header_row)
            .width(Length::Fill)
            .padding([0, space_s]);

        let updates_section: Element<_> = if self.update_items.is_empty() {
            let updates_content: Element<_> = if self.any_checking() {
                widget::row::with_capacity(2)
                    .push(widget::icon::from_name("process-working-symbolic"))
                    .push(widget::text::body(fl!("updates-checking")))
                    .align_y(Alignment::Center)
                    .spacing(space_s)
                    .into()
            } else {
                widget::text::body(self.updates_output()).into()
            };

            let updates_body = widget::scrollable(
                widget::container(updates_content)
                    .width(Length::Fill)
                    .align_x(Horizontal::Center)
                    .padding(space_s),
            )
            .height(Length::Fill);

            widget::column::with_capacity(1)
                .push(widget::container(updates_body).width(Length::Fill).align_x(Horizontal::Center))
                .spacing(space_s)
                .width(Length::Fill)
                .into()
        } else {
            let selected_count = self
                .update_items
                .iter()
                .filter(|item| item.checked)
                .count();
            let total_updates = self.update_items.len();

            if self.show_update_details {
                widget::container(self.update_details_view())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                let mut updates_header = widget::column::with_capacity(3);
                if let Some(banner) = self.running_updates_banner() {
                    updates_header = updates_header.push(banner);
                }
                updates_header = updates_header.push(
                        widget::row::with_capacity(4)
                            .push(widget::horizontal_space())
                            .push(
                                widget::button::standard(fl!("updates-check-all"))
                                    .on_press(Message::SelectAllUpdates),
                            )
                            .push(
                                widget::button::standard(fl!("updates-uncheck-all"))
                                    .on_press(Message::SelectNoUpdates),
                            )
                            .align_y(Alignment::Center)
                            .spacing(space_s),
                    );
                updates_header = updates_header.push(
                        widget::row::with_capacity(2)
                            .push(widget::horizontal_space())
                            .push(widget::text::caption(fl!(
                                "updates-selected",
                                selected = selected_count,
                                total = total_updates
                            )))
                            .align_y(Alignment::Center),
                    );
                updates_header = updates_header.spacing(space_s / 2);

                let mut cards = widget::column::with_capacity(self.update_items.len())
                    .width(Length::Fill)
                    .spacing(space_s);

                for item in self.ordered_update_items() {
                    let key = item.key();
                    let title = widget::text::body(Self::update_display_name(&item.entry));
                    let subtitle = widget::text::caption(Self::update_subtitle(&item.entry));
                    let text_block = widget::column::with_capacity(2)
                        .push(title)
                        .push(subtitle)
                        .spacing(space_s / 2);

                    let icon = widget::icon::from_name("package-x-generic-symbolic");
                    let info_row = widget::row::with_capacity(2)
                        .push(icon)
                        .push(text_block)
                        .align_y(Alignment::Center)
                        .spacing(space_s);

                    let info_area = widget::mouse_area(info_row)
                        .on_press(Message::SelectUpdate(key.clone()));

                    let checkbox = widget::checkbox("", item.checked)
                        .on_toggle(move |checked| Message::ToggleUpdate(key.clone(), checked));

                    let card_row = widget::row::with_capacity(2)
                        .push(widget::container(info_area).width(Length::Fill))
                        .push(widget::container(checkbox))
                        .align_y(Alignment::Center)
                        .spacing(space_s);

                    let content = widget::container(card_row)
                        .width(Length::Fill)
                        .padding(space_s);

                    let mut card_body = widget::column::with_capacity(2)
                        .push(content)
                        .spacing(0);

                    if let Some(progress_value) = self.update_progress_value(item) {
                        let progress_bar = widget::progress_bar(0.0..=100.0, progress_value)
                            .height(Length::Fixed(4.0));
                        card_body = card_body.push(progress_bar);
                    }

                    let card = widget::container(card_body)
                        .width(Length::Fill)
                        .class(cosmic::theme::Container::ContextDrawer);
                    cards = cards.push(card);
                }

                let updates_body = widget::scrollable(
                    widget::container(cards)
                        .width(Length::Fill)
                        .padding(space_s),
                )
                .height(Length::Fill);

                widget::column::with_capacity(2)
                    .push(widget::container(updates_header).width(Length::Fill))
                    .push(widget::container(updates_body).width(Length::Fill))
                    .spacing(space_s)
                    .width(Length::Fill)
                    .into()
            }
        };

        let body = widget::column::with_capacity(4)
            .push(widget::container(updates_section).width(Length::Fill))
            .spacing(space_s)
            .width(Length::Fill);

        let mut content = widget::column::with_capacity(6)
            .push(header)
            .push(body)
            .spacing(space_s)
            .height(Length::Fill);

        if self.reboot_prompt {
            let reboot_button = widget::button::suggested(fl!("reboot-now"));
            let reboot_button = if self.reboot_in_progress {
                reboot_button
            } else {
                reboot_button.on_press(Message::RebootNow)
            };

            let reboot_bar = widget::row::with_capacity(3)
                .push(widget::text::body(fl!("reboot-prompt")))
                .push(reboot_button)
                .push(widget::button::standard(fl!("reboot-later")).on_press(Message::RebootDismiss))
                .align_y(Alignment::Center)
                .spacing(space_s);

            content = content.push(widget::container(reboot_bar).padding(space_s));
        }

        widget::container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(space_s)
            .into()
    }

    /// Register subscriptions for this application.
    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subs = vec![self
            .core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config))];
        if self.any_running() {
            subs.push(
                cosmic::iced::time::every(Duration::from_millis(
                    RUNNING_PROGRESS_TICK_MS,
                ))
                .map(|_| Message::ProgressTick),
            );
        }
        Subscription::batch(subs)
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::DetectComplete(results) => {
                for (id, available) in results {
                    if let Some(state) = self.managers.get_mut(&id) {
                        state.available = available;
                    }
                }

                return self.start_checks(false);
            }
            Message::CheckComplete(id, outcome) => {
                if let Some(state) = self.managers.get_mut(&id) {
                    state.checking = false;
                    state.check_output = outcome.output.clone();
                    if let Some(err) = outcome.error.as_ref() {
                        state.status = TaskStatus::Failed(err.clone());
                    } else {
                        state.update_available = outcome.has_updates;
                    }
                }

                if outcome.error.is_none() {
                    let entries = update::parse_updates(id, &outcome.output);
                    self.set_update_entries(id, entries);
                }

                if self.selected_manager == Some(id) {
                    if let Some(state) = self.managers.get(&id) {
                        if state.last_output.is_empty() {
                            self.log_text = state.check_output.clone();
                        }
                    }
                }
            }
            Message::CheckUpdates => {
                return self.start_checks(true);
            }
            Message::RunManager(id) => {
                self.run_all_queue.clear();
                self.run_all_active = false;
                self.run_all_output.clear();
                return self.start_manager_update(id);
            }
            Message::RunAll => {
                self.run_all_queue.clear();
                self.run_all_active = true;
                self.run_all_output.clear();
                self.selected_manager = None;
                self.log_text = fl!("status-running");
                self.enqueue_updates(true);
                if let Some(next_id) = self.run_all_queue.pop_front() {
                    return self.start_manager_update(next_id);
                }
            }
            Message::UpdateComplete(id, outcome) => {
                if let Some(state) = self.managers.get_mut(&id) {
                    state.status = outcome.status.clone();
                    state.last_output = outcome.output.clone();
                    if matches!(outcome.status, TaskStatus::Success) {
                        state.update_available = false;
                    }
                    state.progress = None;
                }
                if let Ok(mut map) = self.progress_tracker.lock() {
                    map.remove(&id);
                }

                if id == ManagerId::Apt && matches!(outcome.status, TaskStatus::Success) {
                    self.reboot_prompt = update::apt_has_updates(&outcome.output);
                }

                if matches!(outcome.status, TaskStatus::Success) && !self.run_all_active {
                    self.remove_completed_updates(id);
                }

                if self.run_all_active {
                    let header = format!("\n== {} ==\n", id.spec().label);
                    self.run_all_output.push_str(&header);
                    self.run_all_output.push_str(&outcome.output);
                    self.log_text = self.run_all_output.clone();
                } else if self.selected_manager == Some(id) || self.selected_manager.is_none() {
                    self.selected_manager = Some(id);
                    self.log_text = outcome.output;
                }

                if let Some(next_id) = self.run_all_queue.pop_front() {
                    return self.start_manager_update(next_id);
                }

                if self.run_all_active {
                    self.run_all_active = false;
                    self.manager_run_selection.clear();
                    return self.start_checks(true);
                }
            }
            Message::SelectManager(id) => {
                if let Some(state) = self.managers.get(&id) {
                    self.selected_manager = Some(id);
                    if !state.last_output.is_empty() {
                        self.log_text = state.last_output.clone();
                    } else {
                        self.log_text = state.check_output.clone();
                    }
                }
                self.context_page = ContextPage::Logs;
                self.core.window.show_context = true;
            }
            Message::ToggleUpdate(key, checked) => {
                if let Some(item) = self
                    .update_items
                    .iter_mut()
                    .find(|item| item.key() == key)
                {
                    item.checked = checked;
                }
            }
            Message::SelectUpdate(key) => {
                self.selected_update = Some(key);
                self.show_update_details = true;
            }
            Message::SelectDetailsTab(tab) => {
                self.details_tab = tab;
            }
            Message::ShowUpdatesList => {
                self.show_update_details = false;
            }
            Message::SelectAllUpdates => {
                for item in &mut self.update_items {
                    item.checked = true;
                }
            }
            Message::SelectNoUpdates => {
                for item in &mut self.update_items {
                    item.checked = false;
                }
            }
            Message::RebootNow => {
                self.reboot_in_progress = true;
                return Task::perform(update::reboot_system(), Message::RebootFinished)
                    .map(cosmic::Action::App);
            }
            Message::RebootDismiss => {
                self.reboot_prompt = false;
            }
            Message::RebootFinished(result) => {
                self.reboot_in_progress = false;
                if let Err(err) = result {
                    self.log_text = format!("{}: {err}", fl!("reboot-failed"));
                }
            }
            Message::ToggleContextPage(context_page) => {
                if self.context_page == context_page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = context_page;
                    self.core.window.show_context = true;
                }
            }
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::ProgressTick => {
                if self.any_running() {
                    self.sync_progress_from_tracker();
                    self.advance_progress_phase();
                }
            }
            Message::LaunchUrl(url) => match open::that_detached(&url) {
                Ok(()) => {}
                Err(err) => {
                    eprintln!("failed to open {url:?}: {err}");
                }
            },
        }
        Task::none()
    }
}

impl AppModel {
    /// Updates the header and window titles.
    pub fn update_title(&mut self) -> Task<cosmic::Action<Message>> {
        let window_title = fl!("app-title");
        if let Some(id) = self.core.main_window_id() {
            self.set_window_title(window_title, id)
        } else {
            Task::none()
        }
    }

    fn set_running(&mut self, id: ManagerId) {
        let supports_live = self.supports_live_progress(id);
        if let Some(state) = self.managers.get_mut(&id) {
            state.status = TaskStatus::Running;
            state.last_output = fl!("status-running");
            state.progress = if supports_live {
                Some(0.0)
            } else {
                None
            };
        }
    }

    fn supports_live_progress(&self, id: ManagerId) -> bool {
        match id {
            ManagerId::Apt => cfg!(feature = "packagekit"),
            ManagerId::Flatpak => cfg!(feature = "flatpak"),
            _ => false,
        }
    }

    fn start_manager_update(
        &mut self,
        id: ManagerId,
    ) -> Task<cosmic::Action<Message>> {
        self.set_running(id);
        let selected = self.selected_updates_for_manager(id);
        let progress = Some(self.progress_tracker.clone());
        if self.supports_update_selection(id) && !selected.is_empty() {
            let selection = selected
                .iter()
                .map(|entry| UpdateKey {
                    manager: id,
                    id: entry.id.clone(),
                })
                .collect();
            self.manager_run_selection.insert(id, Some(selection));
            Task::perform(
                update::run_manager_with_progress(id, Some(selected), progress),
                move |outcome| Message::UpdateComplete(id, outcome),
            )
            .map(cosmic::Action::App)
        } else {
            if self.supports_update_selection(id) {
                self.manager_run_selection.insert(id, None);
            } else {
                self.manager_run_selection.remove(&id);
            }
            Task::perform(
                update::run_manager_with_progress(id, None, progress),
                move |outcome| Message::UpdateComplete(id, outcome),
            )
            .map(cosmic::Action::App)
        }
    }

    fn start_checks(&mut self, force_privileged: bool) -> Task<cosmic::Action<Message>> {
        let mut tasks = Vec::new();

        for id in ManagerId::all() {
            if let Some(state) = self.managers.get_mut(id) {
                if state.available {
                    state.checking = true;
                    let manager_id = *id;
                    tasks.push(
                        Task::perform(update::check_manager(manager_id, force_privileged), move |outcome| {
                            Message::CheckComplete(manager_id, outcome)
                        })
                        .map(cosmic::Action::App),
                    );
                }
            }
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    fn enqueue_updates(&mut self, force_all: bool) {
        for id in ManagerId::all() {
            if let Some(state) = self.managers.get(id) {
                if !state.available || matches!(state.status, TaskStatus::Running) {
                    continue;
                }

                if self.supports_update_selection(*id) {
                    if self.has_selected_updates(*id) {
                        self.run_all_queue.push_back(*id);
                    }
                    continue;
                }

                if force_all || state.update_available {
                    self.run_all_queue.push_back(*id);
                }
            }
        }
    }

    fn manager_counts(&self) -> (usize, usize) {
        let total = self.managers.len();
        let available = self.managers.values().filter(|state| state.available).count();
        (available, total)
    }

    fn any_running(&self) -> bool {
        self.managers
            .values()
            .any(|state| matches!(state.status, TaskStatus::Running))
    }

    fn is_manager_running(&self, id: ManagerId) -> bool {
        self.managers
            .get(&id)
            .is_some_and(|state| matches!(state.status, TaskStatus::Running))
    }

    fn running_manager_summary(&self) -> Option<String> {
        let labels: Vec<&str> = self
            .managers
            .iter()
            .filter_map(|(id, state)| {
                if matches!(state.status, TaskStatus::Running) {
                    Some(id.spec().label)
                } else {
                    None
                }
            })
            .collect();

        if labels.is_empty() {
            None
        } else {
            Some(labels.join(", "))
        }
    }

    fn running_updates_banner(&self) -> Option<Element<'_, Message>> {
        let managers = self.running_manager_summary()?;
        let space_s = cosmic::theme::spacing().space_s;
        let banner = widget::row::with_capacity(2)
            .push(widget::icon::from_name("process-working-symbolic"))
            .push(widget::text::caption(fl!(
                "updates-running",
                managers = managers
            )))
            .align_y(Alignment::Center)
            .spacing(space_s / 2);

        Some(
            widget::container(banner)
                .width(Length::Fill)
                .align_x(Horizontal::Center)
                .into(),
        )
    }

    fn any_checking(&self) -> bool {
        self.managers.values().any(|state| state.checking)
    }

    fn any_updates_available(&self) -> bool {
        self.managers.values().any(|state| state.update_available)
    }

    fn updates_output(&self) -> String {
        let mut output = String::new();

        for id in ManagerId::all() {
            let state = match self.managers.get(id) {
                Some(state) => state,
                None => continue,
            };

            if !state.available || !state.update_available {
                continue;
            }

            output.push_str(&format!("== {} ==\n", id.spec().label));
            if state.check_output.trim().is_empty() {
                output.push_str(&fl!("updates-available"));
                output.push('\n');
            } else {
                output.push_str(&state.check_output);
                if !state.check_output.ends_with('\n') {
                    output.push('\n');
                }
            }
            output.push('\n');
        }

        if output.trim().is_empty() {
            fl!("system-up-to-date")
        } else {
            output
        }
    }

    fn supports_update_selection(&self, id: ManagerId) -> bool {
        matches!(id, ManagerId::Apt | ManagerId::Flatpak)
    }

    fn has_selected_updates(&self, id: ManagerId) -> bool {
        self.update_items.iter().any(|item| {
            item.entry.manager == id && item.checked
        })
    }

    fn selected_updates_for_manager(&self, id: ManagerId) -> Vec<UpdateEntry> {
        self.update_items
            .iter()
            .filter(|item| item.entry.manager == id && item.checked)
            .map(|item| item.entry.clone())
            .collect()
    }

    fn ordered_update_items(&self) -> Vec<&UpdateItem> {
        let mut items: Vec<(usize, &UpdateItem)> =
            self.update_items.iter().enumerate().collect();
        items.sort_by(|(left_index, left), (right_index, right)| {
            let left_running = self.is_manager_running(left.entry.manager);
            let right_running = self.is_manager_running(right.entry.manager);
            if left_running != right_running {
                return right_running.cmp(&left_running);
            }
            left_index.cmp(right_index)
        });
        items.into_iter().map(|(_, item)| item).collect()
    }

    fn set_update_entries(&mut self, id: ManagerId, entries: Vec<UpdateEntry>) {
        self.update_items
            .retain(|item| item.entry.manager != id);

        for entry in entries {
            self.update_items.push(UpdateItem {
                entry,
                checked: true,
            });
        }

        self.prune_selected_update();
    }

    fn remove_completed_updates(&mut self, id: ManagerId) {
        let selection = if self.supports_update_selection(id) {
            self.manager_run_selection.remove(&id).unwrap_or(None)
        } else {
            None
        };

        match selection {
            Some(keys) => {
                let key_set: HashSet<UpdateKey> = keys.into_iter().collect();
                self.update_items.retain(|item| {
                    item.entry.manager != id || !key_set.contains(&item.key())
                });
            }
            None => {
                self.update_items
                    .retain(|item| item.entry.manager != id);
            }
        }

        self.prune_selected_update();

        if let Some(state) = self.managers.get_mut(&id) {
            state.update_available = self
                .update_items
                .iter()
                .any(|item| item.entry.manager == id);
        }
    }

    fn prune_selected_update(&mut self) {
        if let Some(selected) = &self.selected_update {
            let still_exists = self.update_items.iter().any(|item| item.key() == *selected);
            if !still_exists {
                self.selected_update = None;
                self.show_update_details = false;
            }
        }
    }

    fn details_text(&self) -> String {
        let Some(selected) = &self.selected_update else {
            return fl!("details-none");
        };

        let Some(item) = self
            .update_items
            .iter()
            .find(|item| item.key() == *selected)
        else {
            return fl!("details-none");
        };

        match self.details_tab {
            DetailsTab::Information => {
                let mut info = Vec::new();
                info.push(format!(
                    "{}: {}",
                    fl!("details-manager"),
                    item.entry.manager.spec().label
                ));
                info.push(format!(
                    "{}: {}",
                    fl!("details-name"),
                    Self::update_display_name(&item.entry)
                ));
                if let Some(current) = &item.entry.current_version {
                    info.push(format!("{}: {}", fl!("details-current"), current));
                }
                if let Some(new) = &item.entry.new_version {
                    info.push(format!("{}: {}", fl!("details-new"), new));
                }
                if let Some(scope) = item.entry.scope {
                    let scope_label = match scope {
                        update::InstallScope::User => fl!("details-scope-user"),
                        update::InstallScope::System => fl!("details-scope-system"),
                    };
                    info.push(format!("{}: {}", fl!("details-scope"), scope_label));
                }
                info.join("\n")
            }
            DetailsTab::Packages => {
                if item.entry.manager == ManagerId::Flatpak {
                    format!("{}: {}", fl!("details-ref"), item.entry.id.as_str())
                } else {
                    format!("{}: {}", fl!("details-package"), item.entry.id.as_str())
                }
            }
            DetailsTab::Changelog => fl!("details-changelog-unavailable"),
        }
    }

    fn update_display_name(entry: &UpdateEntry) -> &str {
        let name = entry.name.trim();
        if name.is_empty() {
            entry.id.as_str()
        } else {
            name
        }
    }

    fn update_subtitle(entry: &UpdateEntry) -> String {
        let manager = entry.manager.spec().label;
        let current = entry
            .current_version
            .clone()
            .unwrap_or_else(|| fl!("version-unknown"));
        let new = entry
            .new_version
            .clone()
            .unwrap_or_else(|| fl!("version-unknown"));

        format!(
            "{} · {} {} -> {} {}",
            manager,
            fl!("details-current"),
            current,
            fl!("details-new"),
            new
        )
    }

    fn update_progress_value(&self, item: &UpdateItem) -> Option<f32> {
        let Some(state) = self.managers.get(&item.entry.manager) else {
            return None;
        };

        if !matches!(state.status, TaskStatus::Running) {
            return None;
        }

        if let Some(selection) = self
            .manager_run_selection
            .get(&item.entry.manager)
            .and_then(|selection| selection.as_ref())
        {
            if !selection.contains(&item.key()) {
                return None;
            }
        } else if !item.checked {
            return None;
        }

        if let Some(progress) = state.progress {
            Some(progress)
        } else {
            Some(self.running_progress_value())
        }
    }

    fn sync_progress_from_tracker(&mut self) {
        let Ok(progress_map) = self.progress_tracker.lock() else {
            return;
        };

        for (id, state) in self.managers.iter_mut() {
            if matches!(state.status, TaskStatus::Running) {
                state.progress = progress_map.get(id).copied();
            } else {
                state.progress = None;
            }
        }
    }

    fn advance_progress_phase(&mut self) {
        let span = RUNNING_PROGRESS_MAX - RUNNING_PROGRESS_MIN;
        if span <= 0.0 {
            return;
        }
        let cycle = span * 2.0;
        self.progress_phase = (self.progress_phase + RUNNING_PROGRESS_STEP) % cycle;
    }

    fn running_progress_value(&self) -> f32 {
        let span = RUNNING_PROGRESS_MAX - RUNNING_PROGRESS_MIN;
        if span <= 0.0 {
            return RUNNING_PROGRESS_MIN;
        }
        let cycle = span * 2.0;
        let phase = self.progress_phase % cycle;
        let offset = if phase <= span { phase } else { cycle - phase };
        RUNNING_PROGRESS_MIN + offset
    }

    fn details_tabs(&self) -> Element<'_, Message> {
        let info_button = if self.details_tab == DetailsTab::Information {
            widget::button::suggested(fl!("details-information"))
        } else {
            widget::button::standard(fl!("details-information"))
                .on_press(Message::SelectDetailsTab(DetailsTab::Information))
        };
        let packages_button = if self.details_tab == DetailsTab::Packages {
            widget::button::suggested(fl!("details-packages"))
        } else {
            widget::button::standard(fl!("details-packages"))
                .on_press(Message::SelectDetailsTab(DetailsTab::Packages))
        };
        let changelog_button = if self.details_tab == DetailsTab::Changelog {
            widget::button::suggested(fl!("details-changelog"))
        } else {
            widget::button::standard(fl!("details-changelog"))
                .on_press(Message::SelectDetailsTab(DetailsTab::Changelog))
        };

        widget::row::with_capacity(3)
            .push(info_button)
            .push(packages_button)
            .push(changelog_button)
            .spacing(cosmic::theme::spacing().space_s)
            .into()
    }

    fn update_details_view(&self) -> Element<'_, Message> {
        let space_s = cosmic::theme::spacing().space_s;
        let (title, subtitle) = self
            .selected_update
            .as_ref()
            .and_then(|selected| {
                self.update_items
                    .iter()
                    .find(|item| item.key() == *selected)
            })
            .map(|item| {
                (
                    Self::update_display_name(&item.entry).to_string(),
                    Self::update_subtitle(&item.entry),
                )
            })
            .unwrap_or_else(|| (fl!("details-title"), String::new()));

        let details_body = widget::scrollable(
            widget::container(widget::text::body(self.details_text()))
                .width(Length::Fill)
                .padding(space_s),
        )
        .height(Length::Fill);

        let header = widget::column::with_capacity(2)
            .push(widget::text::title3(title))
            .push(widget::text::caption(subtitle))
            .spacing(space_s / 2);

        widget::column::with_capacity(4)
            .push(header)
            .push(self.details_tabs())
            .push(details_body)
            .spacing(space_s)
            .width(Length::Fill)
            .into()
    }

    fn back_button(&self) -> Element<'_, Message> {
        let style = cosmic::theme::Button::Custom {
            active: Box::new(|_focused, theme| {
                let cosmic = theme.cosmic();
                let mut style = widget::button::Style::new();
                style.text_color = Some(Color::from(cosmic.accent_text_color()));
                style.icon_color = Some(Color::from(cosmic.accent_text_color()));
                style.border_radius = cosmic.corner_radii.radius_xl.into();
                style
            }),
            disabled: Box::new(|theme| {
                let cosmic = theme.cosmic();
                let mut style = widget::button::Style::new();
                style.text_color = Some(Color::from(cosmic.accent_text_color()));
                style.icon_color = Some(Color::from(cosmic.accent_text_color()));
                style.border_radius = cosmic.corner_radii.radius_xl.into();
                style
            }),
            hovered: Box::new(|_focused, theme| {
                let cosmic = theme.cosmic();
                let mut style = widget::button::Style::new();
                style.background = Some(Background::Color(cosmic.accent_button.hover.into()));
                style.text_color = Some(Color::from(cosmic.accent_button.on));
                style.icon_color = Some(Color::from(cosmic.accent_button.on));
                style.border_radius = cosmic.corner_radii.radius_xl.into();
                style
            }),
            pressed: Box::new(|_focused, theme| {
                let cosmic = theme.cosmic();
                let mut style = widget::button::Style::new();
                style.text_color = Some(Color::from(cosmic.accent_text_color()));
                style.icon_color = Some(Color::from(cosmic.accent_text_color()));
                style.border_radius = cosmic.corner_radii.radius_xl.into();
                style
            }),
        };

        let label = widget::row::with_capacity(2)
            .push(widget::icon::from_name("go-previous-symbolic"))
            .push(widget::text::body(fl!("back")))
            .align_y(Alignment::Center)
            .spacing(cosmic::theme::spacing().space_s);

        widget::button::custom(label)
            .class(style)
            .on_press(Message::ShowUpdatesList)
            .into()
    }

    fn summoners_drawer_content(&self) -> Element<'_, Message> {
        let space_s = cosmic::theme::spacing().space_s;
        let (available_count, _total_count) = self.manager_counts();

        let summary = widget::container(widget::text::body(fl!(
            "detected-summary",
            available = available_count
        )))
        .width(Length::Fill)
        .align_x(Horizontal::Center);

        let mut manager_list = widget::column::with_capacity(self.managers.len())
            .spacing(space_s)
            .width(Length::Fill)
            .align_x(Horizontal::Center);

        for id in ManagerId::all() {
            let spec = id.spec();
            let state = self.managers.get(id).expect("manager state missing");

            if !state.available {
                continue;
            }

            let mut status_label = if state.checking {
                fl!("status-checking")
            } else if state.update_available {
                fl!("status-updates-available")
            } else {
                match &state.status {
                    TaskStatus::Idle => fl!("status-up-to-date"),
                    TaskStatus::Running => fl!("status-running"),
                    TaskStatus::Success => fl!("status-success"),
                    TaskStatus::Failed(reason) => {
                        if reason.is_empty() {
                            fl!("status-failed")
                        } else {
                            format!("{}: {}", fl!("status-failed"), reason)
                        }
                    }
                }
            };
            if state.requires_privilege {
                status_label.push_str(" · ");
                status_label.push_str(&fl!("requires-auth"));
            }

            let status_widget: Element<_> = if state.checking {
                widget::row::with_capacity(2)
                    .push(widget::icon::from_name("process-working-symbolic"))
                    .push(widget::text::caption(status_label))
                    .align_y(Alignment::Center)
                    .spacing(space_s)
                    .into()
            } else {
                widget::text::caption(status_label).into()
            };

            let update_button = widget::button::standard(fl!("update"));
            let update_button = if !matches!(state.status, TaskStatus::Running) {
                update_button.on_press(Message::RunManager(*id))
            } else {
                update_button
            };

            let view_button = widget::button::standard(fl!("view-log"))
                .on_press(Message::SelectManager(*id));

            let info_row = widget::row::with_capacity(2)
                .push(
                    widget::container(widget::text::body(spec.label))
                        .width(Length::FillPortion(1)),
                )
                .push(widget::container(status_widget).width(Length::FillPortion(4)))
                .align_y(Alignment::Center)
                .spacing(space_s);

            let actions_row = widget::row::with_capacity(3)
                .push(widget::horizontal_space())
                .push(update_button)
                .push(view_button)
                .align_y(Alignment::Center)
                .spacing(space_s);

            let row = widget::column::with_capacity(2)
                .push(info_row)
                .push(actions_row)
                .spacing(space_s / 2)
                .width(Length::Fill);

            manager_list = manager_list.push(row);
        }

        widget::column::with_capacity(2)
            .push(summary)
            .push(widget::container(manager_list))
            .spacing(space_s)
            .width(Length::Fill)
            .into()
    }
}

impl UpdateItem {
    fn key(&self) -> UpdateKey {
        UpdateKey {
            manager: self.entry.manager,
            id: self.entry.id.clone(),
        }
    }
}

    /// The context page to display in the context drawer.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Summoners,
    Logs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Summoners,
    Logs,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Summoners => Message::ToggleContextPage(ContextPage::Summoners),
            MenuAction::Logs => Message::ToggleContextPage(ContextPage::Logs),
        }
    }
}
