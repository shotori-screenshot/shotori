//! # Command-line interface (clap)
//!
//! Subcommands follow standard Wayland screenshot-tool conventions:
//! - no subcommand / `gui`: interactive selection & annotation overlay
//!   (optionally pre-selecting all screens, the focused screen, a named
//!   output, the focused window, or an explicit region, and optionally
//!   pre-arming an annotation tool)
//! - `full`: non-interactive capture (all screens by default, or scoped to
//!   focused screen / output / window / region; clipboard, file, or stdout)
//! - `ocr`: on-device text recognition from a screen capture or image file
//! - `pin`: float an image file or screen capture as an always-on-top pin
//! - `outputs`: inspect connected Wayland outputs and their geometry
//! - `tray`: resident StatusNotifierItem tray launcher
//! - `settings`: regular toplevel settings window (optionally on a page)
//!
//! The internal child-process entry points (`--notify`, `--bench`, `--perf`,
//! and `--clipboard-daemon` on Linux) are matched on `argv[1]` in `main.rs`
//! BEFORE clap runs — they carry free-form trailing arguments and must not
//! be validated as flags.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "shotori",
    version,
    about = "Screenshot tool — Wayland-first",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Preinstalled theme (shotori-dark, shotori-light, catppuccin-*,
    /// dracula, gruvbox-*, nord, rose-pine*, tokyo-night*, solarized-light,
    /// github-light) or a TOML theme file
    #[arg(long, value_name = "NAME|FILE")]
    pub theme: Option<String>,

    /// Print resolved colors (both palettes for auto) and exit
    #[arg(long)]
    pub print_theme: bool,

    /// Skip the config-file theme/settings auto-pickup (~/.config/shotori/)
    #[arg(long)]
    pub no_config: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Interactive selection and annotation overlay (default when no subcommand is given)
    Gui(GuiArgs),

    /// Capture without an overlay (all screens by default, or a chosen target)
    Full(FullArgs),

    /// Recognize text on-device from a screen capture or image file
    Ocr(OcrArgs),

    /// Float an image file or screen capture as an always-on-top desktop pin
    Pin(PinArgs),

    /// List connected Wayland outputs and their geometry
    Outputs(OutputsArgs),

    /// Stay resident as a tray icon; activating it starts a screenshot
    Tray,

    /// Open the settings window (optionally on a specific section)
    Settings(SettingsArgs),
}

/// Interactive overlay flags (`shotori gui [OPTIONS]`).
#[derive(Args, Debug, Default, Clone)]
#[command(group(
    ArgGroup::new("target")
        .args(["full", "screen", "output", "region", "window"])
        .multiple(false)
))]
pub struct GuiArgs {
    /// Pre-select every connected screen (ready to annotate immediately)
    #[arg(short, long)]
    pub full: bool,

    /// Pre-select the currently focused screen
    #[arg(short, long)]
    pub screen: bool,

    /// Pre-select a specific output by name (see `shotori outputs`)
    #[arg(short, long, value_name = "NAME")]
    pub output: Option<String>,

    /// Pre-select a desktop logical region (`x,y,w,h`, `x,y wxh`, or `wxh+x+y`)
    #[arg(short, long, value_name = "GEOM", allow_hyphen_values = true)]
    pub region: Option<RegionSpec>,

    /// Pre-select the currently focused window (via compositor IPC)
    #[arg(short, long)]
    pub window: bool,

    /// Wait SECONDS before freezing screens
    #[arg(short, long, default_value_t = 0.0, value_parser = parse_delay)]
    pub delay: f32,

    /// Pre-arm an annotation tool once a selection is active
    #[arg(short, long, value_enum, value_name = "TOOL")]
    pub tool: Option<ToolArg>,
}

impl GuiArgs {
    /// Initial pre-selection requested on the CLI (`None` = normal idle overlay).
    pub fn target(&self) -> Option<SelectionTarget> {
        if self.full {
            Some(SelectionTarget::AllScreens)
        } else if self.screen {
            Some(SelectionTarget::FocusedScreen)
        } else if let Some(output) = &self.output {
            Some(SelectionTarget::Output(output.clone()))
        } else if let Some(region) = self.region {
            Some(SelectionTarget::Region(region))
        } else if self.window {
            Some(SelectionTarget::FocusedWindow)
        } else {
            None
        }
    }
}

/// Non-interactive capture flags (`shotori full [OPTIONS]`).
#[derive(Args, Debug, Default, Clone)]
#[command(group(
    ArgGroup::new("target")
        .args(["full", "screen", "output", "region", "window"])
        .multiple(false)
))]
pub struct FullArgs {
    /// Capture every connected screen (the default when no target flag is given)
    #[arg(short, long)]
    pub full: bool,

    /// Capture only the currently focused screen
    #[arg(short, long)]
    pub screen: bool,

    /// Capture only the named output (see `shotori outputs`)
    #[arg(short, long, value_name = "NAME")]
    pub output: Option<String>,

    /// Capture a desktop logical region (`x,y,w,h`, `x,y wxh`, or `wxh+x+y`)
    #[arg(short, long, value_name = "GEOM", allow_hyphen_values = true)]
    pub region: Option<RegionSpec>,

    /// Capture only the currently focused window (via compositor IPC)
    #[arg(short, long)]
    pub window: bool,

    /// Copy the PNG image to the clipboard (default when neither --path nor --stdout is given)
    #[arg(short, long)]
    pub clipboard: bool,

    /// Write PNG to this file (`-` for stdout), or into this directory with an auto-generated name
    #[arg(short, long, value_name = "FILE|DIR")]
    pub path: Option<PathBuf>,

    /// Write encoded PNG bytes to standard output
    #[arg(long)]
    pub stdout: bool,

    /// Wait SECONDS before capturing
    #[arg(short, long, default_value_t = 0.0, value_parser = parse_delay)]
    pub delay: f32,

    /// Suppress desktop notifications
    #[arg(short = 'n', long)]
    pub no_notify: bool,
}

impl FullArgs {
    pub fn target(&self) -> SelectionTarget {
        if self.screen {
            SelectionTarget::FocusedScreen
        } else if let Some(output) = &self.output {
            SelectionTarget::Output(output.clone())
        } else if let Some(region) = self.region {
            SelectionTarget::Region(region)
        } else if self.window {
            SelectionTarget::FocusedWindow
        } else {
            SelectionTarget::AllScreens
        }
    }
}

/// Non-interactive OCR flags (`shotori ocr [OPTIONS] [IMAGE]`).
#[derive(Args, Debug, Default, Clone)]
#[command(group(
    ArgGroup::new("target")
        .args(["full", "screen", "output", "region", "window"])
        .multiple(false)
))]
pub struct OcrArgs {
    /// Image file to recognize (`-` for stdin); omit to capture from the screen
    #[arg(value_name = "IMAGE")]
    pub image: Option<PathBuf>,

    /// Capture every connected screen (default when IMAGE is omitted)
    #[arg(short, long, conflicts_with = "image")]
    pub full: bool,

    /// Capture only the currently focused screen
    #[arg(short, long, conflicts_with = "image")]
    pub screen: bool,

    /// Capture only the named output (see `shotori outputs`)
    #[arg(short, long, value_name = "NAME", conflicts_with = "image")]
    pub output: Option<String>,

    /// Capture a desktop logical region (`x,y,w,h`, `x,y wxh`, or `wxh+x+y`)
    #[arg(
        short,
        long,
        value_name = "GEOM",
        allow_hyphen_values = true,
        conflicts_with = "image"
    )]
    pub region: Option<RegionSpec>,

    /// Capture only the currently focused window (via compositor IPC)
    #[arg(short, long, conflicts_with = "image")]
    pub window: bool,

    /// Wait SECONDS before capturing from the screen
    #[arg(
        short,
        long,
        default_value_t = 0.0,
        value_parser = parse_delay,
        conflicts_with = "image"
    )]
    pub delay: f32,

    /// Copy recognized text to the clipboard
    #[arg(short, long)]
    pub clipboard: bool,

    /// Write recognized text to FILE (`-` for stdout)
    #[arg(short, long, value_name = "FILE")]
    pub path: Option<PathBuf>,

    /// Print recognized text to standard output (default when neither --clipboard nor --path is given)
    #[arg(long)]
    pub stdout: bool,

    /// Suppress desktop notifications
    #[arg(short = 'n', long)]
    pub no_notify: bool,

    /// Download missing OCR models automatically on first use
    #[arg(long)]
    pub download: bool,
}

impl OcrArgs {
    pub fn screen_target(&self) -> SelectionTarget {
        if self.screen {
            SelectionTarget::FocusedScreen
        } else if let Some(output) = &self.output {
            SelectionTarget::Output(output.clone())
        } else if let Some(region) = self.region {
            SelectionTarget::Region(region)
        } else if self.window {
            SelectionTarget::FocusedWindow
        } else {
            SelectionTarget::AllScreens
        }
    }
}

/// Desktop floating-pin flags (`shotori pin [OPTIONS] [IMAGE]`).
#[derive(Args, Debug, Default, Clone)]
#[command(
    group(
        ArgGroup::new("source")
            .args(["image", "full", "screen", "output", "region", "window"])
            .required(true)
            .multiple(true)
    ),
    group(
        ArgGroup::new("capture_target")
            .args(["full", "screen", "output", "region", "window"])
            .multiple(false)
    )
)]
pub struct PinArgs {
    /// Image file to pin (`-` for stdin)
    #[arg(value_name = "IMAGE")]
    pub image: Option<PathBuf>,

    /// Capture and pin every connected screen
    #[arg(short, long, conflicts_with = "image")]
    pub full: bool,

    /// Capture and pin the currently focused screen
    #[arg(short, long, conflicts_with = "image")]
    pub screen: bool,

    /// Output name: captures that output, or centers IMAGE on it when IMAGE is given
    #[arg(short, long, value_name = "NAME")]
    pub output: Option<String>,

    /// Capture and pin a desktop logical region (`x,y,w,h`, `x,y wxh`, or `wxh+x+y`)
    #[arg(
        short,
        long,
        value_name = "GEOM",
        allow_hyphen_values = true,
        conflicts_with = "image"
    )]
    pub region: Option<RegionSpec>,

    /// Capture and pin the currently focused window (via compositor IPC)
    #[arg(short, long, conflicts_with = "image")]
    pub window: bool,

    /// Wait SECONDS before capturing from the screen
    #[arg(
        short,
        long,
        default_value_t = 0.0,
        value_parser = parse_delay,
        conflicts_with = "image"
    )]
    pub delay: f32,

    /// Place the pin's top-left corner at global logical coordinates `x,y`
    #[arg(long, value_name = "X,Y", allow_hyphen_values = true)]
    pub at: Option<PointSpec>,
}

impl PinArgs {
    /// Screen capture target when `image` is omitted.
    pub fn capture_target(&self) -> Option<SelectionTarget> {
        if self.image.is_some() {
            return None;
        }
        if self.full {
            Some(SelectionTarget::AllScreens)
        } else if self.screen {
            Some(SelectionTarget::FocusedScreen)
        } else if let Some(output) = &self.output {
            Some(SelectionTarget::Output(output.clone()))
        } else if let Some(region) = self.region {
            Some(SelectionTarget::Region(region))
        } else if self.window {
            Some(SelectionTarget::FocusedWindow)
        } else {
            None
        }
    }
}

/// Output inspection flags (`shotori outputs [OPTIONS]`).
#[derive(Args, Debug, Default, Clone)]
pub struct OutputsArgs {
    /// Format output metadata as JSON
    #[arg(short, long)]
    pub json: bool,
}

/// Settings window flags (`shotori settings [PAGE]`).
#[derive(Args, Debug, Default, Clone)]
pub struct SettingsArgs {
    /// Initial settings section to open
    #[arg(value_enum, value_name = "PAGE")]
    pub page: Option<SettingsPage>,
}

/// Which desktop region a command targets.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectionTarget {
    AllScreens,
    FocusedScreen,
    Output(String),
    Region(RegionSpec),
    FocusedWindow,
}

impl SelectionTarget {
    /// Whether resolving this target benefits from querying compositor IPC.
    pub fn uses_compositor_ipc(&self) -> bool {
        matches!(
            self,
            Self::AllScreens | Self::FocusedScreen | Self::FocusedWindow
        )
    }
}

/// Annotation tool selectable via `shotori gui --tool <TOOL>`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolArg {
    Select,
    #[value(alias = "rect")]
    Rectangle,
    Ellipse,
    Line,
    Polyline,
    Arrow,
    Number,
    Text,
    Pencil,
    Highlighter,
    Mosaic,
    Blur,
    Magnifier,
    Spotlight,
    Eraser,
}

/// Settings window section selectable via `shotori settings [PAGE]`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Appearance,
    #[value(alias = "toolbar")]
    Capture,
    #[value(alias = "shortcuts")]
    Keys,
    Ocr,
    #[value(alias = "general")]
    Advanced,
    About,
}

/// Desktop logical rectangle parsed from CLI geometry strings.
///
/// Accepted forms:
/// - `x,y,w,h` (Shotori CSV, e.g. `100,200,800,600` or `-720,-100,1920,1080`)
/// - `x,y wxh` (`slurp` output, e.g. `100,200 800x600`)
/// - `wxh+x+y` / `wxh-x+y` (X11 geometry, e.g. `800x600+100+200` or `800x600-720-100`)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionSpec {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl fmt::Display for RegionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

impl FromStr for RegionSpec {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let s = raw.trim();
        let parsed = if let Some((xy, wh)) = s.split_once(char::is_whitespace) {
            let (x, y) = parse_pair(xy.trim(), ',')?;
            let (w, h) = parse_dims(wh.trim())?;
            (x, y, w, h)
        } else if s.contains(',') {
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() != 4 {
                return Err(format!(
                    "expected 4 comma-separated numbers 'x,y,w,h', got '{raw}'"
                ));
            }
            (
                parse_num(parts[0])?,
                parse_num(parts[1])?,
                parse_num(parts[2])?,
                parse_num(parts[3])?,
            )
        } else if let Some(geom) = parse_x11_geometry(s) {
            geom
        } else {
            return Err(format!(
                "invalid region '{raw}' (expected 'x,y,w,h', 'x,y wxh', or 'wxh+x+y')"
            ));
        };

        let (x, y, width, height) = parsed;
        if width < 2.0 || height < 2.0 {
            return Err(format!(
                "region size must be at least 2x2 logical pixels (got {width}x{height})"
            ));
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
}

/// Desktop logical point parsed from `x,y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointSpec {
    pub x: f32,
    pub y: f32,
}

impl FromStr for PointSpec {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (x, y) = parse_pair(raw.trim(), ',')?;
        Ok(Self { x, y })
    }
}

fn parse_delay(raw: &str) -> Result<f32, String> {
    let val: f32 = raw
        .trim()
        .parse()
        .map_err(|_| format!("invalid delay '{raw}' (expected non-negative seconds)"))?;
    if !val.is_finite() || val < 0.0 {
        return Err(format!("delay must be finite and >= 0 (got '{raw}')"));
    }
    Ok(val)
}

fn parse_num(raw: &str) -> Result<f32, String> {
    let v: f32 = raw
        .trim()
        .parse()
        .map_err(|_| format!("invalid number '{}'", raw.trim()))?;
    if !v.is_finite() {
        return Err(format!("coordinate '{}' must be finite", raw.trim()));
    }
    Ok(v)
}

fn parse_pair(raw: &str, sep: char) -> Result<(f32, f32), String> {
    let (a, b) = raw
        .split_once(sep)
        .ok_or_else(|| format!("expected 'x{sep}y', got '{raw}'"))?;
    Ok((parse_num(a)?, parse_num(b)?))
}

fn parse_dims(raw: &str) -> Result<(f32, f32), String> {
    let (w, h) = raw
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected 'wxh', got '{raw}'"))?;
    Ok((parse_num(w)?, parse_num(h)?))
}

/// Parse `WxH+X+Y` (where `+X`/`+Y` may be `+` or `-` signed offsets).
fn parse_x11_geometry(s: &str) -> Option<(f32, f32, f32, f32)> {
    let (w_str, rest) = s.split_once(['x', 'X'])?;
    let sign_1 = rest.find(['+', '-'])?;
    let h_str = &rest[..sign_1];
    let xy_rest = &rest[sign_1..];
    // Find the second sign after the first sign character
    let sign_2 = xy_rest[1..].find(['+', '-'])? + 1;
    let x_str = &xy_rest[..sign_2];
    let y_str = &xy_rest[sign_2..];
    Some((
        parse_num(x_str).ok()?,
        parse_num(y_str).ok()?,
        parse_num(w_str).ok()?,
        parse_num(h_str).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn cli(v: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("shotori").chain(v.iter().copied())).unwrap()
    }

    #[test]
    fn no_subcommand_is_a_gui_run() {
        let a = cli(&[]);
        assert!(a.command.is_none());
        assert_eq!(a.theme, None);
    }

    #[test]
    fn gui_supports_fullscreen_and_tool_preselection() {
        let a = cli(&["gui", "--full", "--tool", "arrow", "-d", "1.5"]);
        match a.command {
            Some(Command::Gui(g)) => {
                assert_eq!(g.target(), Some(SelectionTarget::AllScreens));
                assert_eq!(g.tool, Some(ToolArg::Arrow));
                assert_eq!(g.delay, 1.5);
            }
            other => panic!("{other:?}"),
        }

        // Short flags and rect alias
        let b = cli(&["gui", "-f", "-t", "rect"]);
        match b.command {
            Some(Command::Gui(g)) => {
                assert_eq!(g.target(), Some(SelectionTarget::AllScreens));
                assert_eq!(g.tool, Some(ToolArg::Rectangle));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn gui_targets_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["shotori", "gui", "--full", "--screen"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "gui", "--full", "-o", "DP-1"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "gui", "--window", "-r", "0,0,10,10"]).is_err());
    }

    #[test]
    fn region_parses_csv_slurp_x11_and_negative_origins() {
        assert_eq!(
            "10,20,300,200".parse::<RegionSpec>().unwrap(),
            RegionSpec {
                x: 10.0,
                y: 20.0,
                width: 300.0,
                height: 200.0,
            }
        );
        // Negative multi-monitor global coordinates via clap without `--`
        let a = cli(&["gui", "-r", "-720,-100,1080,1920"]);
        match a.command {
            Some(Command::Gui(g)) => assert_eq!(
                g.region,
                Some(RegionSpec {
                    x: -720.0,
                    y: -100.0,
                    width: 1080.0,
                    height: 1920.0,
                })
            ),
            other => panic!("{other:?}"),
        }
        // slurp default format: "x,y wxh"
        assert_eq!(
            "-720,-100 1080x1920".parse::<RegionSpec>().unwrap(),
            RegionSpec {
                x: -720.0,
                y: -100.0,
                width: 1080.0,
                height: 1920.0,
            }
        );
        // X11 geometry format: "wxh+x+y" and negative offsets
        assert_eq!(
            "800x600+100+200".parse::<RegionSpec>().unwrap(),
            RegionSpec {
                x: 100.0,
                y: 200.0,
                width: 800.0,
                height: 600.0,
            }
        );
        assert_eq!(
            "800x600-720-100".parse::<RegionSpec>().unwrap(),
            RegionSpec {
                x: -720.0,
                y: -100.0,
                width: 800.0,
                height: 600.0,
            }
        );
        // Degenerate dimensions are rejected
        assert!("0,0,1,100".parse::<RegionSpec>().is_err());
        assert!("0,0,-10,100".parse::<RegionSpec>().is_err());
        assert!("garbage".parse::<RegionSpec>().is_err());
    }

    #[test]
    fn full_flags_and_targets() {
        let a = cli(&[
            "full",
            "--clipboard",
            "-p",
            "/tmp",
            "-d",
            "2.5",
            "-o",
            "DP-1",
            "--stdout",
            "--no-notify",
        ]);
        match a.command {
            Some(Command::Full(f)) => {
                assert!(f.clipboard);
                assert_eq!(f.path.as_deref(), Some(Path::new("/tmp")));
                assert_eq!(f.delay, 2.5);
                assert_eq!(f.target(), SelectionTarget::Output("DP-1".into()));
                assert!(f.stdout);
                assert!(f.no_notify);
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["shotori", "full", "-s", "-o", "DP-1"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "full", "-d", "-1"]).is_err());
    }

    #[test]
    fn ocr_supports_image_and_screen_targets() {
        let img = cli(&["ocr", "scan.png", "--clipboard", "--no-notify"]);
        match img.command {
            Some(Command::Ocr(o)) => {
                assert_eq!(o.image.as_deref(), Some(Path::new("scan.png")));
                assert!(o.clipboard);
                assert!(o.no_notify);
            }
            other => panic!("{other:?}"),
        }
        // Image conflicts with screen-capture flags
        assert!(Cli::try_parse_from(["shotori", "ocr", "scan.png", "--screen"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "ocr", "scan.png", "-d", "1"]).is_err());

        let scr = cli(&["ocr", "--screen", "--download"]);
        match scr.command {
            Some(Command::Ocr(o)) => {
                assert!(o.image.is_none());
                assert_eq!(o.screen_target(), SelectionTarget::FocusedScreen);
                assert!(o.download);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pin_requires_source_and_allows_output_placement_for_image() {
        // Bare `shotori pin` must ask for an image or capture flag
        assert!(Cli::try_parse_from(["shotori", "pin"]).is_err());

        // Image + output placement + negative `--at`
        let img = cli(&["pin", "diagram.png", "-o", "DP-1", "--at", "-300,50"]);
        match img.command {
            Some(Command::Pin(p)) => {
                assert_eq!(p.image.as_deref(), Some(Path::new("diagram.png")));
                assert_eq!(p.output.as_deref(), Some("DP-1"));
                assert_eq!(p.capture_target(), None);
                assert_eq!(p.at, Some(PointSpec { x: -300.0, y: 50.0 }));
            }
            other => panic!("{other:?}"),
        }

        // Capture-and-pin output without IMAGE
        let cap = cli(&["pin", "-o", "DP-1"]);
        match cap.command {
            Some(Command::Pin(p)) => {
                assert!(p.image.is_none());
                assert_eq!(
                    p.capture_target(),
                    Some(SelectionTarget::Output("DP-1".into()))
                );
            }
            other => panic!("{other:?}"),
        }

        // Non-output capture flags still conflict with IMAGE
        assert!(Cli::try_parse_from(["shotori", "pin", "diagram.png", "--screen"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "pin", "--full", "--screen"]).is_err());
    }

    #[test]
    fn outputs_and_settings_subcommands() {
        let out = cli(&["outputs", "--json"]);
        assert!(matches!(
            out.command,
            Some(Command::Outputs(OutputsArgs { json: true }))
        ));

        assert!(matches!(
            cli(&["settings"]).command,
            Some(Command::Settings(SettingsArgs { page: None }))
        ));
        assert!(matches!(
            cli(&["settings", "ocr"]).command,
            Some(Command::Settings(SettingsArgs {
                page: Some(SettingsPage::Ocr)
            }))
        ));
        assert!(matches!(
            cli(&["settings", "toolbar"]).command,
            Some(Command::Settings(SettingsArgs {
                page: Some(SettingsPage::Capture)
            }))
        ));
        assert!(matches!(
            cli(&["settings", "shortcuts"]).command,
            Some(Command::Settings(SettingsArgs {
                page: Some(SettingsPage::Keys)
            }))
        ));
    }

    #[test]
    fn theme_flags_carry_into_subcommands() {
        let a = cli(&["--theme", "light", "--no-config", "full", "-c"]);
        assert_eq!(a.theme.as_deref(), Some("light"));
        assert!(a.no_config);
        assert!(matches!(
            a.command,
            Some(Command::Full(FullArgs {
                clipboard: true,
                ..
            }))
        ));
    }

    #[test]
    fn gui_is_explicit() {
        assert!(matches!(cli(&["gui"]).command, Some(Command::Gui(_))));
    }

    #[test]
    fn unknown_arguments_error() {
        assert!(Cli::try_parse_from(["shotori", "--them"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "fullscreen"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "full", "-x"]).is_err());
    }

    /// The generated help must stay renderable (guards the derive and ArgGroups).
    #[test]
    fn help_renders() {
        let mut buf = Vec::new();
        <Cli as clap::CommandFactory>::command()
            .write_help(&mut buf)
            .unwrap();
        let help = String::from_utf8(buf).unwrap();
        assert!(help.contains("Usage: shotori"));
        assert!(help.contains("--theme"));
        for sub in ["gui", "full", "ocr", "pin", "outputs", "tray", "settings"] {
            assert!(
                help.contains(sub),
                "missing subcommand {sub} in help:\n{help}"
            );
        }
    }
}
