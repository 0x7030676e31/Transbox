use crate::{Args, instance, session};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use transbox_storage::{OpenStore, Recovery, Store, Unlocked};
use zeroize::{Zeroize, Zeroizing};

const ACCENT: Color32 = Color32::from_rgb(104, 218, 191);
const MUTED: Color32 = Color32::from_rgb(155, 165, 181);

enum Event {
    Unlocked(Store, Result<Unlocked>),
    Progress(String),
    Started,
    Finished(Store, Result<()>),
    Reset(Store, Result<()>),
}

pub fn run(args: Args, running: Arc<AtomicBool>) -> Result<()> {
    let directory = args
        .data_dir
        .clone()
        .map(Ok)
        .unwrap_or_else(transbox_storage::default_directory)?;
    let store = match Store::open(&directory)? {
        OpenStore::Acquired(store) => store,
        OpenStore::Busy => return instance::focus_existing(&directory),
    };
    let control = instance::Control::start(store.directory())?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Transbox")
            .with_inner_size([440.0, 500.0])
            .with_min_inner_size([440.0, 500.0])
            .with_resizable(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Transbox",
        options,
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.style_of(egui::Theme::Dark)).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.panel_fill = Color32::from_rgb(19, 24, 32);
            style.visuals.window_fill = style.visuals.panel_fill;
            style.visuals.extreme_bg_color = Color32::from_rgb(12, 17, 24);
            style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.3);
            style.spacing.item_spacing = egui::vec2(10.0, 12.0);
            cc.egui_ctx.set_style_of(egui::Theme::Dark, style);
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            Ok(Box::new(UnlockWindow::new(store, args, running, control)))
        }),
    )
    .map_err(|error| anyhow::anyhow!("Opening unlock window: {error}"))
}

struct UnlockWindow {
    // Stop IPC before dropping the store lock: a new controller must not have
    // its descriptor removed by the preceding instance's cleanup.
    control: instance::Control,
    store: Option<Store>,
    args: Args,
    running: Arc<AtomicBool>,
    sender: mpsc::Sender<Event>,
    receiver: mpsc::Receiver<Event>,
    password: String,
    confirmation: String,
    reset_text: String,
    error: Option<String>,
    status: String,
    busy: bool,
    browser_active: bool,
    closing: bool,
    reset_screen: bool,
    backup: bool,
    pending: Option<(Store, Unlocked)>,
    confirm_discard: bool,
    focus_password: bool,
}

impl UnlockWindow {
    fn new(store: Store, args: Args, running: Arc<AtomicBool>, control: instance::Control) -> Self {
        let backup = !store.exists() && store.has_backup();
        let (sender, receiver) = mpsc::channel();
        Self {
            store: Some(store),
            args,
            running,
            control,
            sender,
            receiver,
            password: String::new(),
            confirmation: String::new(),
            reset_text: String::new(),
            error: None,
            status: String::new(),
            busy: false,
            browser_active: false,
            closing: false,
            reset_screen: false,
            backup,
            pending: None,
            confirm_discard: false,
            focus_password: true,
        }
    }

    fn is_setup(&self) -> bool {
        self.store
            .as_ref()
            .is_some_and(|s| !s.exists() && !s.has_backup() && !s.has_working_files())
    }

    fn unlock(&mut self, ctx: &egui::Context) {
        let setup = self.is_setup();
        if setup && self.password.chars().count() < 12 {
            self.error = Some("Use at least 12 characters for your password.".into());
            return;
        }
        if setup && self.password != self.confirmation {
            self.error = Some("The passwords do not match.".into());
            return;
        }
        let Some(store) = self.store.take() else {
            return;
        };
        let password = Zeroizing::new(std::mem::take(&mut self.password));
        self.confirmation.zeroize();
        self.confirmation.clear();
        self.busy = true;
        self.error = None;
        self.status = if setup {
            "Creating protected storage…"
        } else {
            "Unlocking your browser…"
        }
        .into();
        let (sender, ctx, backup) = (self.sender.clone(), ctx.clone(), self.backup);
        thread::spawn(move || {
            let result = if setup {
                store.create(&password)
            } else {
                store.unlock(&password, backup)
            };
            drop(password);
            let _ = sender.send(Event::Unlocked(store, result));
            ctx.request_repaint();
        });
    }

    fn start_session(
        &mut self,
        store: Store,
        unlocked: Unlocked,
        recovery: Recovery,
        ctx: &egui::Context,
    ) {
        self.busy = true;
        self.error = None;
        self.status = "Preparing your browser…".into();
        self.running.store(true, Ordering::Relaxed);
        let (sender, ctx, args, running, focus) = (
            self.sender.clone(),
            ctx.clone(),
            self.args.clone(),
            self.running.clone(),
            self.control.focus.clone(),
        );
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let profile = store.prepare(&unlocked, recovery)?;
                let outcome = session::run(
                    &args,
                    &profile,
                    running,
                    focus,
                    |text| {
                        let _ = sender.send(Event::Progress(text.to_owned()));
                        ctx.request_repaint();
                    },
                    || {
                        let _ = sender.send(Event::Started);
                        ctx.request_repaint();
                    },
                );
                if !outcome.safe_to_save {
                    return outcome.result.context("Working files remain unencrypted. Close Chromium and recover the session on next unlock.");
                }
                let _ = sender.send(Event::Progress("Saving and locking your browser…".into()));
                ctx.request_repaint();
                store.save(&unlocked).context("Could not finish locking storage. Working files remain unencrypted; unlock again to recover them.")?;
                outcome.result
            })();
            drop(unlocked);
            let _ = sender.send(Event::Finished(store, result));
            ctx.request_repaint();
        });
    }

    fn events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Unlocked(store, result) => {
                    self.busy = false;
                    match result {
                        Ok(unlocked) if self.closing => {
                            self.store = Some(store);
                            drop(unlocked);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Ok(unlocked) if store.needs_recovery() => {
                            self.pending = Some((store, unlocked));
                        }
                        Ok(unlocked) => self.start_session(store, unlocked, Recovery::Saved, ctx),
                        Err(error) => {
                            self.store = Some(store);
                            self.error = Some(format!("{error:#}"));
                            self.focus_password = true;
                        }
                    }
                }
                Event::Progress(text) => self.status = text,
                Event::Started => {
                    self.browser_active = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                Event::Finished(store, result) => {
                    let clean_cancel = self.closing && !store.has_working_files();
                    self.store = Some(store);
                    self.busy = false;
                    self.browser_active = false;
                    match result {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(_) if clean_cancel => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close)
                        }
                        Err(error) => {
                            self.error = Some(format!("{error:#}"));
                            self.closing = false;
                            self.focus_password = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                    }
                }
                Event::Reset(store, result) => {
                    self.store = Some(store);
                    self.busy = false;
                    self.reset_screen = false;
                    self.backup = false;
                    self.password.zeroize();
                    self.password.clear();
                    self.confirmation.zeroize();
                    self.confirmation.clear();
                    self.reset_text.clear();
                    self.focus_password = true;
                    self.error = result.err().map(|e| format!("{e:#}"));
                }
            }
        }
    }

    fn primary(ui: &mut egui::Ui, text: &str) -> bool {
        ui.add_sized(
            [ui.available_width(), 44.0],
            egui::Button::new(
                RichText::new(text)
                    .strong()
                    .color(Color32::from_rgb(12, 28, 26)),
            )
            .fill(ACCENT)
            .corner_radius(8),
        )
        .clicked()
    }

    fn password_field(
        ui: &mut egui::Ui,
        value: &mut String,
        id: &str,
        hint: &str,
    ) -> egui::Response {
        let response = ui.add_sized(
            [ui.available_width(), 34.0],
            egui::TextEdit::singleline(value)
                .id_salt(id)
                .password(true)
                .char_limit(256)
                .hint_text(hint),
        );
        // Password values must not be recoverable through a later Ctrl+Z in
        // an apparently empty field after an unlock attempt or reset.
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), response.id) {
            state.clear_undoer();
            state.store(ui.ctx(), response.id);
        }
        response
    }
}

impl eframe::App for UnlockWindow {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::from_rgb(19, 24, 32).to_normalized_gamma_f32()
    }
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
        if !self.browser_active && self.control.focus.swap(false, Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.busy {
            self.closing = true;
            self.running.store(false, Ordering::Relaxed);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.status = "Closing and saving…".into();
        }
        if !self.running.load(Ordering::Relaxed) && !self.busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui.add_space(25.0);
        egui::Frame::new().inner_margin(egui::Margin::symmetric(30, 0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("TRANSBOX").size(14.0).strong().color(ACCENT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(RichText::new(env!("CARGO_PKG_VERSION")).size(11.0).color(MUTED)); });
            });
            ui.add_space(25.0);
            if self.busy {
                ui.label(RichText::new(if self.browser_active { "Your browser is open" } else { "Transbox" }).size(27.0).strong());
                ui.add_space(18.0);
                ui.horizontal(|ui| { ui.spinner(); ui.label(&self.status); });
                ui.add_space(16.0);
                ui.label(RichText::new("This window disappears when the browser opens.").color(MUTED));
            } else if self.pending.is_some() {
                ui.label(RichText::new("Recover your session").size(27.0).strong());
                ui.add_space(10.0);
                ui.label("An interrupted shutdown left unencrypted working files. Recover that session, or discard it and use the last saved copy.");
                ui.add_space(15.0);
                if Self::primary(ui, "Recover unsaved session") {
                    let (store, unlocked) = self.pending.take().unwrap(); self.start_session(store, unlocked, Recovery::Unsaved, &ctx);
                }
                if !self.confirm_discard {
                    if ui.button("Use last saved copy instead").clicked() { self.confirm_discard = true; }
                } else {
                    ui.label(RichText::new("Unsaved changes will be permanently discarded.").color(Color32::LIGHT_RED));
                    if ui.button("Discard unsaved changes & continue").clicked() {
                        let (store, unlocked) = self.pending.take().unwrap(); self.confirm_discard = false;
                        self.start_session(store, unlocked, Recovery::Saved, &ctx);
                    }
                }
                if ui.button("Cancel").clicked() {
                    let (store, unlocked) = self.pending.take().unwrap(); drop(unlocked); self.store = Some(store); self.confirm_discard = false;
                }
            } else if self.reset_screen {
                ui.label(RichText::new("Reset local storage").size(27.0).strong());
                ui.add_space(10.0);
                ui.label("This deletes your browser history, saved logins, settings, encrypted backup and any interrupted session. It cannot be undone.");
                ui.add_space(10.0);
                ui.label("Type RESET to confirm:");
                ui.add(egui::TextEdit::singleline(&mut self.reset_text).desired_width(f32::INFINITY));
                let reset = ui.add_enabled(self.reset_text == "RESET", egui::Button::new("Delete local browser data").fill(Color32::from_rgb(140, 42, 52))).clicked();
                if reset {
                    let store = self.store.take().unwrap(); let (sender, ctx) = (self.sender.clone(), ctx.clone());
                    self.busy = true; self.status = "Resetting local storage…".into();
                    thread::spawn(move || { let result = store.reset(); let _ = sender.send(Event::Reset(store, result)); ctx.request_repaint(); });
                }
                if ui.button("Cancel").clicked() { self.reset_screen = false; self.reset_text.clear(); }
            } else {
                let setup = self.is_setup();
                ui.label(RichText::new(if setup { "Create local storage" } else { "Unlock local storage" }).size(29.0).strong());
                ui.add_space(8.0);
                ui.label(RichText::new(if setup { "Choose a password for your local browser storage." } else { "Unlock your history, saved logins and settings." }).color(MUTED));
                ui.add_space(16.0);
                ui.label("Password");
                let input = Self::password_field(ui, &mut self.password, "storage-password", if setup { "At least 12 characters" } else { "Your storage password" });
                if self.focus_password { input.request_focus(); self.focus_password = false; }
                if setup {
                    ui.label("Confirm password");
                    Self::password_field(ui, &mut self.confirmation, "storage-password-confirmation", "Repeat password");
                }
                if !setup && self.store.as_ref().is_some_and(Store::has_backup) { ui.checkbox(&mut self.backup, "Use previous saved copy"); }
                if self.store.as_ref().is_some_and(Store::needs_recovery) {
                    ui.label(RichText::new("Unencrypted working files remain from an interrupted session. Unlock to recover, or reset storage.").size(12.0).color(Color32::LIGHT_RED));
                }
                if self.store.as_ref().is_some_and(Store::pending_cleanup) {
                    ui.label(RichText::new("The last session was saved, but working files remain. Unlock to finish cleanup.").size(12.0).color(Color32::LIGHT_RED));
                }
                ui.add_space(8.0);
                if Self::primary(ui, if setup { "Create & open browser" } else { "Unlock & open browser" }) || ui.input(|i| i.key_pressed(egui::Key::Enter)) { self.unlock(&ctx); }
                if !setup && ui.small_button("Reset local storage…").clicked() { self.reset_screen = true; self.error = None; }
            }
            if let Some(error) = &self.error {
                ui.add_space(10.0);
                egui::ScrollArea::vertical().max_height(95.0).show(ui, |ui| { ui.label(RichText::new(error).size(12.0).color(Color32::LIGHT_RED)); });
            }
        });
    }
}

impl Drop for UnlockWindow {
    fn drop(&mut self) {
        self.password.zeroize();
        self.confirmation.zeroize();
    }
}
