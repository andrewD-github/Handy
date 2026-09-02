use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone, Default)]
#[command(name = "handy", about = "Handy - Speech to Text")]
pub struct CliArgs {
    /// Start with the main window hidden
    #[arg(long)]
    pub start_hidden: bool,

    /// Disable the system tray icon
    #[arg(long)]
    pub no_tray: bool,

    /// Toggle transcription on/off (sent to running instance)
    #[arg(long)]
    pub toggle_transcription: bool,

    /// Toggle transcription with post-processing on/off (sent to running instance)
    #[arg(long)]
    pub toggle_post_process: bool,

    /// Cancel the current operation (sent to running instance)
    #[arg(long)]
    pub cancel: bool,

    /// Enable debug mode with verbose logging
    #[arg(long)]
    pub debug: bool,

    /// Transcribe this WAV (16 kHz mono) headlessly and exit. Runs the same
    /// batch transcription path as the app — no mic, no VAD, no download
    /// (the model must already be installed).
    #[arg(short = 'f', long, value_name = "WAV")]
    pub transcribe_file: Option<PathBuf>,

    /// Model id to load for --transcribe-file (default: the selected model).
    #[arg(long)]
    pub model: Option<String>,

    /// Hard-select the compute device for --transcribe-file by its registry
    /// index (see --list-devices). Omit to use the persisted accelerator
    /// setting. transcribe-cpp (whisper-family) models only.
    #[arg(long, value_name = "N")]
    pub device_index: Option<usize>,

    /// List the transcribe-cpp compute devices (with indices) and exit.
    #[arg(long)]
    pub list_devices: bool,

    /// List the available models (with ids) and exit. Pass an id to --model.
    /// Honors --json for machine-readable output.
    #[arg(long)]
    pub list_models: bool,

    /// Repeat the transcription N times (best_ms reports the fastest run).
    #[arg(long, value_name = "N")]
    pub repeat: Option<usize>,

    /// Feed --transcribe-file through the production streaming worker and
    /// include every committed/tentative snapshot in JSON output.
    #[arg(long, requires = "transcribe_file")]
    pub stream_replay: bool,

    /// Pace --stream-replay according to the WAV duration instead of feeding
    /// all chunks as quickly as possible.
    #[arg(long, requires = "stream_replay")]
    pub realtime: bool,

    /// Override consecutive agreement passes for committed streaming text.
    /// Replay-lab only; normal app streaming uses the engine default.
    #[arg(long, requires = "stream_replay", value_name = "N")]
    pub stable_prefix_agreement: Option<u32>,

    /// Emit --transcribe-file results as JSON.
    #[arg(long)]
    pub json: bool,

    /// Write JSON output to a file. This keeps replay automation available in
    /// the normal Windows GUI build, where stdout is intentionally detached.
    #[arg(long, value_name = "FILE", requires = "json")]
    pub json_output: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::CliArgs;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn streaming_replay_cli_requires_explicit_flags() {
        let args = CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "saved.wav",
            "--stream-replay",
            "--realtime",
        ])
        .unwrap();

        assert!(args.stream_replay);
        assert!(args.realtime);
    }

    #[test]
    fn streaming_replay_accepts_an_explicit_stable_prefix_agreement() {
        let args = CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "saved.wav",
            "--stream-replay",
            "--stable-prefix-agreement",
            "2",
        ])
        .unwrap();

        assert_eq!(args.stable_prefix_agreement, Some(2));
    }

    #[test]
    fn json_output_requires_json_mode() {
        assert!(CliArgs::try_parse_from(["handy", "--json-output", "result.json"]).is_err());
        let args = CliArgs::try_parse_from([
            "handy",
            "--transcribe-file",
            "saved.wav",
            "--json",
            "--json-output",
            "result.json",
        ])
        .unwrap();
        assert_eq!(args.json_output, Some(PathBuf::from("result.json")));
    }
}
