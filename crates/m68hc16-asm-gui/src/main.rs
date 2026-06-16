#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

use eframe::egui;
use m68hc16_asm::{assemble, AssembleRequest, BinOptions, Outputs};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "M68HC16 Assembler",
        options,
        Box::new(|_cc| Ok(Box::new(AssemblerApp::new()))),
    )
}

struct AssemblerApp {
    input: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    // Which outputs to generate.
    gen_obj: bool,
    gen_s19: bool,
    gen_lst: bool,
    gen_bin: bool,
    // `.bin` window controls (hex text). Defaults: fill 0xFF, size 0x40000 (256 KB),
    // base 0 — a 256 KB ROM window.
    bin_fill: String,
    bin_size: String,
    bin_base: String,
    log: String,
}

impl AssemblerApp {
    fn new() -> Self {
        AssemblerApp {
            input: None,
            output_dir: None,
            gen_obj: true,
            gen_s19: true,
            gen_lst: true,
            gen_bin: false,
            bin_fill: "FF".to_string(),
            bin_size: "40000".to_string(),
            bin_base: "0".to_string(),
            log: String::new(),
        }
    }

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

        // Parse the .bin window fields (only matters when .bin is checked).
        let bin = if self.gen_bin {
            match (parse_hex(&self.bin_fill), parse_hex(&self.bin_size), parse_hex(&self.bin_base)) {
                (Some(f), Some(s), Some(b)) if f <= 0xFF && s > 0 => BinOptions { fill: f as u8, base: b, size: s },
                (Some(f), _, _) if f > 0xFF => {
                    self.log.push_str("Fill must be a single byte (00-FF)\n");
                    return;
                }
                (_, Some(0), _) => {
                    self.log.push_str("Size must be greater than 0\n");
                    return;
                }
                _ => {
                    self.log.push_str("Fill / Size / Base must be hex values\n");
                    return;
                }
            }
        } else {
            BinOptions::default()
        };

        let req = AssembleRequest {
            input,
            output_dir,
            outputs: Outputs { obj: self.gen_obj, s19: self.gen_s19, lst: self.gen_lst, bin: self.gen_bin },
            bin,
        };
        let result = assemble(&req);
        for diag in &result.diagnostics {
            self.log.push_str(&format!("{diag}\n"));
        }
        if result.has_errors() {
            self.log.push_str("assemble: FAILED\n");
            return;
        }
        self.log.push_str("assemble: ok\n");
        let o = &result.outputs;
        let written: Vec<_> = [&o.object, &o.s_record, &o.binary, &o.listing].into_iter().flatten().collect();
        if written.is_empty() {
            self.log.push_str("(no output files selected)\n");
        } else {
            self.log.push_str("wrote:\n");
            for p in written {
                self.log.push_str(&format!("  {}\n", p.display()));
            }
        }
    }
}

/// Parse a hex integer, tolerating a `0x`/`$` prefix and surrounding space.
fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X").trim_start_matches('$');
    u32::from_str_radix(t, 16).ok()
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
                ui.label(self.input.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<none>".to_string()));
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

            ui.add_space(6.0);
            ui.separator();
            ui.label("Generate:");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.gen_obj, ".OBJ");
                ui.checkbox(&mut self.gen_s19, ".S19");
                ui.checkbox(&mut self.gen_lst, ".LST");
                ui.checkbox(&mut self.gen_bin, ".BIN");
            });

            // .bin window controls, shown only when .BIN is selected.
            ui.add_enabled_ui(self.gen_bin, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fill (hex byte):");
                    ui.add(egui::TextEdit::singleline(&mut self.bin_fill).desired_width(48.0));
                    ui.add_space(12.0);
                    ui.label("Base (hex):");
                    ui.add(egui::TextEdit::singleline(&mut self.bin_base).desired_width(80.0));
                    ui.add_space(12.0);
                    ui.label("Size (hex):");
                    ui.add(egui::TextEdit::singleline(&mut self.bin_size).desired_width(80.0));
                });
                ui.label(
                    egui::RichText::new("default: base 0, size 40000 (256 KB), fill FF")
                        .small()
                        .weak(),
                );
            });

            ui.add_space(8.0);
            if ui.button("Assemble").clicked() {
                self.run();
            }

            ui.separator();
            ui.label("Log");
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.log.as_str())
                        .desired_rows(14)
                        .desired_width(f32::INFINITY),
                );
            });
        });
    }
}
