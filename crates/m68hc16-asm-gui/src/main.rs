#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

use eframe::egui;
use m68hc16_asm::{assemble, AssembleRequest};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "M68HC16 Assembler",
        options,
        Box::new(|_cc| Ok(Box::<AssemblerApp>::default())),
    )
}

#[derive(Default)]
struct AssemblerApp {
    input: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    /// Also write the raw binary image (`<stem>.bin`) alongside the `.S19`.
    emit_binary: bool,
    log: String,
}

impl AssemblerApp {
    fn pick_input(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Assembly source", &["asm", "ASM", "s", "S"])
            .pick_file()
        {
            self.input = Some(path);
        }
    }

    fn pick_output_dir(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.output_dir = Some(path);
        }
    }

    fn run(&mut self) {
        self.log.clear();
        let Some(input) = self.input.clone() else {
            self.log.push_str("no input file selected\n");
            return;
        };
        let output_dir = self
            .output_dir
            .clone()
            .or_else(|| input.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let req = AssembleRequest { input, output_dir, emit_binary: self.emit_binary };
        let result = assemble(&req);
        for diag in &result.diagnostics {
            self.log.push_str(&format!("{diag}\n"));
        }
        if result.has_errors() {
            self.log.push_str("assemble: FAILED\n");
            return;
        }
        self.log.push_str("assemble: ok\nwrote:\n");
        let o = &result.outputs;
        for path in [&o.object, &o.s_record, &o.binary, &o.listing] {
            if let Some(p) = path {
                self.log.push_str(&format!("  {}\n", p.display()));
            }
        }
    }
}

impl eframe::App for AssemblerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("M68HC16 Assembler");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button("Input .asm…").clicked() {
                    self.pick_input();
                }
                ui.label(
                    self.input
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                );
            });

            ui.horizontal(|ui| {
                if ui.button("Output dir…").clicked() {
                    self.pick_output_dir();
                }
                ui.label(
                    self.output_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<input directory>".to_string()),
                );
            });

            ui.add_space(4.0);
            ui.checkbox(&mut self.emit_binary, "Also write raw binary (.bin)")
                .on_hover_text("Flat memory image from the lowest emitted address, gaps filled 0xFF");

            ui.add_space(8.0);
            if ui.button("Assemble").clicked() {
                self.run();
            }

            ui.separator();
            ui.label("Log");
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.log.as_str())
                        .desired_rows(16)
                        .desired_width(f32::INFINITY),
                );
            });
        });
    }
}
