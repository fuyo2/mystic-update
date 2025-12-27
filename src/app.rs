// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::fl;
use crate::update::{self, ManagerId, TaskStatus, UpdateOutcome};
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::alignment::Horizontal;
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::widget::{self, about::About, menu};
use cosmic::{prelude::*, Task};
use std::collections::{BTreeMap, HashMap, VecDeque};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const APP_ICON: &[u8] =
    include_bytes!("../resources/icons/hicolor/scalable/apps/mystic-update.svg");

#[derive(Clone, Debug)]
struct ManagerState {
    available: bool,
    requires_privilege: bool,
    status: TaskStatus,
    last_output: String,
    check_output: String,
    checking: bool,
    update_available: bool,
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
    /// Show the updates prompt after checks complete.
    updates_prompt: bool,
    /// Allow dismissing the updates prompt for this session.
    updates_prompt_dismissed: bool,
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
    InstallUpdates,
    UpdateComplete(ManagerId, UpdateOutcome),
    SelectManager(ManagerId),
    UpdatesDismiss,
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
            updates_prompt: false,
            updates_prompt_dismissed: false,
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

        let header = widget::container(widget::text::title1(fl!("app-title")))
            .width(Length::Fill)
            .align_x(Horizontal::Left);

        let summary = widget::container(widget::text::body(fl!("detected-summary", available = available_count)))
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

            let row = widget::row::with_capacity(4)
                .push(widget::text::body(spec.label))
                .push(status_widget)
                .push(update_button)
                .push(view_button)
                .align_y(Alignment::Center)
                .spacing(space_s);

            manager_list = manager_list.push(row);
        }

        let updates_title = widget::text::title4(fl!("updates-title"));
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

        let footer = widget::row::with_capacity(4)
            .push(widget::horizontal_space())
            .push(check_updates_button)
            .push(run_all_button)
            .align_y(Alignment::Center)
            .spacing(space_s);

        let body = widget::column::with_capacity(4)
            .push(summary)
            .push(widget::container(manager_list).padding(space_s))
            .push(widget::container(updates_title).width(Length::Fill).align_x(Horizontal::Left))
            .push(widget::container(updates_body).width(Length::Fill).align_x(Horizontal::Center))
            .spacing(space_s)
            .width(Length::Fill);

        let mut content = widget::column::with_capacity(7)
            .push(header)
            .push(body)
            .push(widget::container(footer).padding(space_s))
            .spacing(space_s)
            .height(Length::Fill);

        if self.updates_prompt && !self.updates_prompt_dismissed {
            let install_button = widget::button::suggested(fl!("install-updates"));
            let install_button = if any_running {
                install_button
            } else {
                install_button.on_press(Message::InstallUpdates)
            };

            let updates_bar = widget::row::with_capacity(3)
                .push(widget::text::body(fl!("updates-prompt")))
                .push(install_button)
                .push(widget::button::standard(fl!("updates-later")).on_press(Message::UpdatesDismiss))
                .align_y(Alignment::Center)
                .spacing(space_s);

            content = content.push(widget::container(updates_bar).padding(space_s));
        }

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
        Subscription::batch(vec![self
            .core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config))])
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
                    state.update_available = outcome.has_updates;
                    if let Some(err) = outcome.error {
                        state.status = TaskStatus::Failed(err);
                    }
                }

                if !self.updates_prompt_dismissed {
                    self.updates_prompt = self
                        .managers
                        .values()
                        .any(|state| state.update_available);
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
                self.updates_prompt = false;
                return self.start_manager_update(id);
            }
            Message::RunAll => {
                self.run_all_queue.clear();
                self.run_all_active = true;
                self.run_all_output.clear();
                self.selected_manager = None;
                self.log_text = fl!("status-running");
                self.updates_prompt = false;
                self.updates_prompt_dismissed = true;
                self.enqueue_updates(true);
                if let Some(next_id) = self.run_all_queue.pop_front() {
                    return self.start_manager_update(next_id);
                }
            }
            Message::InstallUpdates => {
                self.run_all_queue.clear();
                self.run_all_active = true;
                self.run_all_output.clear();
                self.selected_manager = None;
                self.log_text = fl!("status-running");
                self.updates_prompt = false;
                self.updates_prompt_dismissed = true;
                self.enqueue_updates(false);
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
                }

                if id == ManagerId::Apt && matches!(outcome.status, TaskStatus::Success) {
                    self.reboot_prompt = update::apt_has_updates(&outcome.output);
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
            Message::RebootNow => {
                self.reboot_in_progress = true;
                return Task::perform(update::reboot_system(), Message::RebootFinished)
                    .map(cosmic::Action::App);
            }
            Message::RebootDismiss => {
                self.reboot_prompt = false;
            }
            Message::UpdatesDismiss => {
                self.updates_prompt = false;
                self.updates_prompt_dismissed = true;
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
        if let Some(state) = self.managers.get_mut(&id) {
            state.status = TaskStatus::Running;
            state.last_output = fl!("status-running");
        }
    }

    fn start_manager_update(
        &mut self,
        id: ManagerId,
    ) -> Task<cosmic::Action<Message>> {
        self.set_running(id);
        Task::perform(update::run_manager(id), move |outcome| {
            Message::UpdateComplete(id, outcome)
        })
        .map(cosmic::Action::App)
    }

    fn start_checks(&mut self, force_privileged: bool) -> Task<cosmic::Action<Message>> {
        self.updates_prompt = false;
        self.updates_prompt_dismissed = false;
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
}


    /// The context page to display in the context drawer.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Logs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
    Logs,
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
            MenuAction::Logs => Message::ToggleContextPage(ContextPage::Logs),
        }
    }
}
