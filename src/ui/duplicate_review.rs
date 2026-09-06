use super::ImageSearchApp;
use crate::{
    duplicates::{self, DuplicateReport},
    search_request::SearchSession,
};
use eframe::egui;
use std::sync::mpsc::{Receiver, Sender};

pub(super) struct DuplicateUi {
    pub open: bool,
    session: SearchSession,
    running: bool,
    report: Option<DuplicateReport>,
    selected: usize,
    tx: Sender<(u64, Result<DuplicateReport, String>)>,
    rx: Receiver<(u64, Result<DuplicateReport, String>)>,
}
impl Default for DuplicateUi {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            open: false,
            session: Default::default(),
            running: false,
            report: None,
            selected: 0,
            tx,
            rx,
        }
    }
}
impl ImageSearchApp {
    pub(super) fn show_duplicate_review(&mut self, ctx: &egui::Context) {
        while let Ok((id, result)) = self.duplicate_ui.rx.try_recv() {
            if !self.duplicate_ui.session.finish(id) {
                continue;
            }
            self.duplicate_ui.running = false;
            match result {
                Ok(report) => {
                    self.duplicate_ui.report = Some(report);
                    self.duplicate_ui.selected = 0;
                }
                Err(error) => self.last_error = Some(error),
            }
        }
        if !self.duplicate_ui.open {
            return;
        }
        if self.duplicate_ui.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut open = true;
        egui::Window::new("Duplicate review").open(&mut open).default_size([950.0,650.0]).show(ctx,|ui| {
            ui.label("Exact groups use full-file SHA-256. Visual groups are approximate review suggestions and can overlap exact groups. No files are deleted.");
            if ui.add_enabled(!self.duplicate_ui.running && !self.people_filter_work_pending() && !self.text_search_pending && self.search_text == self.text_search_observed,egui::Button::new("Scan current filters")).clicked() {
                let request=self.duplicate_ui.session.start();
                let tx=self.duplicate_ui.tx.clone();
                let db=self.db_path.clone();
                let eligible=self.search_eligibility();
                self.duplicate_ui.running=true;
                std::thread::spawn(move||{let result=duplicates::scan(&db,eligible.as_ref(),&request).map_err(|e|format!("{e:#}"));let _=tx.send((request.id,result));});
            }
            if self.duplicate_ui.running {
                ui.spinner();
                if ui.button("Cancel").clicked(){self.duplicate_ui.session.cancel();self.duplicate_ui.running=false;}
            }
            let mut paths=Vec::new();
            if let Some(report)=&self.duplicate_ui.report {
                ui.label(format!("{} groups · {} files hashed · {} unavailable/changed files skipped",report.groups.len(),report.hashed,report.skipped));
                egui::ScrollArea::vertical().max_height(180.0).show(ui,|ui|{
                    for (index,group) in report.groups.iter().enumerate(){
                        ui.selectable_value(&mut self.duplicate_ui.selected,index,format!("{} · {} images · {}",if group.exact{"Verified identical bytes"}else{"Similar-looking"},group.paths.len(),group.paths[0].file_name().unwrap_or_default().to_string_lossy()));
                    }
                });
                paths=report.groups.get(self.duplicate_ui.selected).map(|group|group.paths.clone()).unwrap_or_default();
            }
            egui::ScrollArea::both().show(ui,|ui|{
                ui.horizontal_top(|ui|{for path in paths {ui.vertical(|ui|{
                    if let Some(texture)=self.thumbnail(&path){ui.add(egui::Image::new(&texture).max_size(egui::vec2(230.0,230.0)));}
                    ui.add(egui::Label::new(path.display().to_string()).wrap());
                    if ui.button("Open").clicked(){let _=open::that(&path);}
                    if ui.button("Show in Explorer").clicked(){crate::windows_shell::show_in_explorer(path.clone());}
                });}});
            });
        });
        self.duplicate_ui.open = open;
        if !open {
            self.duplicate_ui.session.cancel();
            self.duplicate_ui.running = false;
        }
    }
}
