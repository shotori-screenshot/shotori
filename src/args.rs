//! # Command-line interface (clap)
//!
//! Subcommands follow the usual screenshot-tool CLI convention: no subcommand / `gui` opens the interactive overlay,
//! `full` captures without any UI. Theme flags apply to both modes.
//!
//! The internal child-process entry points (`--notify`, and
//! `--clipboard-daemon` on Linux) are matched on `argv[1]` in `main.rs`
//! BEFORE clap runs — they carry free-form trailing arguments and must
//! not be validated as flags.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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

    /// Skip the config-file theme auto-pickup (~/.config/shotori/theme.toml)
    #[arg(long)]
    pub no_config: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Interactive selection overlay (the default when no subcommand is given)
    Gui,

    /// Capture every screen, no overlay
    Full {
        /// Copy the result to the clipboard
        #[arg(short, long)]
        clipboard: bool,

        /// Write to this file, or into this directory with an
        /// auto-generated name
        #[arg(short, long, value_name = "FILE|DIR")]
        path: Option<PathBuf>,

        /// Wait SECONDS before capturing
        #[arg(short, long, default_value_t = 0.)]
        delay: f32,
    },

    /// Stay resident as a tray icon; activating it starts a screenshot
    Tray,

    /// Open the settings window (toolbar layout; more sections later)
    Settings,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn full_flags() {
        let a = cli(&["full", "--clipboard", "-p", "/tmp", "-d", "2.5"]);
        match a.command {
            Some(Command::Full {
                clipboard,
                path,
                delay,
            }) => {
                assert!(clipboard);
                assert_eq!(path.as_deref(), Some(std::path::Path::new("/tmp")));
                assert_eq!(delay, 2.5);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn theme_flags_carry_into_subcommands() {
        let a = cli(&["--theme", "light", "--no-config", "full", "-c"]);
        assert_eq!(a.theme.as_deref(), Some("light"));
        assert!(a.no_config);
        assert!(matches!(
            a.command,
            Some(Command::Full {
                clipboard: true,
                ..
            })
        ));
    }

    #[test]
    fn gui_is_explicit() {
        assert!(matches!(cli(&["gui"]).command, Some(Command::Gui)));
    }

    #[test]
    fn settings_is_a_subcommand() {
        assert!(matches!(
            cli(&["settings"]).command,
            Some(Command::Settings)
        ));
    }

    #[test]
    fn unknown_arguments_error() {
        assert!(Cli::try_parse_from(["shotori", "--them"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "fullscreen"]).is_err());
        assert!(Cli::try_parse_from(["shotori", "full", "-x"]).is_err());
    }

    /// The generated help must stay renderable (guards the derive).
    #[test]
    fn help_renders() {
        let mut buf = Vec::new();
        <Cli as clap::CommandFactory>::command()
            .write_help(&mut buf)
            .unwrap();
        let help = String::from_utf8(buf).unwrap();
        assert!(help.contains("Usage: shotori"));
        assert!(help.contains("--theme"));
        assert!(help.contains("full"));
    }
}
