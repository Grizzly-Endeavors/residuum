//! Setup subcommand: interactive or flag-driven configuration wizard.

use residuum::config::Config;
use residuum::util::FatalError;

/// Run the `setup` subcommand — interactive or flag-driven config wizard.
pub(super) fn run_setup_command(args: &[String]) -> Result<(), FatalError> {
    use residuum::config::wizard;

    let config_dir = Config::config_dir()?;
    let config_path = config_dir.join("config.toml");

    if config_path.exists() {
        println!("config.toml already exists at {}", config_path.display());
        println!("edit it directly or delete it to re-run setup");
        return Ok(());
    }

    // Check if any flags are present → non-interactive mode
    let tz_flag = super::extract_flag_value(args, "--timezone");
    let provider_flag = super::extract_flag_value(args, "--provider");
    let key_flag = super::extract_flag_value(args, "--api-key");
    let model_flag = super::extract_flag_value(args, "--model");
    let ws_backend_flag = super::extract_flag_value(args, "--web-search-backend");
    let ws_key_flag = super::extract_flag_value(args, "--web-search-api-key");
    let ws_url_flag = super::extract_flag_value(args, "--web-search-base-url");

    let has_flags = tz_flag.is_some()
        || provider_flag.is_some()
        || key_flag.is_some()
        || model_flag.is_some()
        || ws_backend_flag.is_some()
        || ws_key_flag.is_some()
        || ws_url_flag.is_some();

    let answers = if has_flags {
        wizard::from_flags(
            tz_flag.as_deref(),
            provider_flag.as_deref(),
            key_flag.as_deref(),
            model_flag.as_deref(),
            ws_backend_flag.as_deref(),
            ws_key_flag.as_deref(),
            ws_url_flag.as_deref(),
        )?
    } else {
        wizard::run_interactive()?
    };

    // Bootstrap creates the directory + example config
    Config::bootstrap_at_dir(&config_dir)?;
    // Write the wizard-generated config (overwrites the minimal template)
    wizard::write_config(&config_dir, &answers)?;

    // Validate the result
    match Config::load_at(&config_dir) {
        Ok(cfg) => {
            println!("configuration saved to {}", config_path.display());
            println!("  timezone: {}", answers.timezone);
            println!("  model: {}/{}", answers.provider, answers.model);
            if cfg.main.first().and_then(|s| s.api_key.as_ref()).is_some() {
                println!("  api key: configured");
            }
            if let Some(ref backend) = answers.web_search_backend {
                println!("  web search: {backend}");
            }
        }
        Err(err) => {
            println!("warning: config was written but validation failed: {err}");
            println!("you may need to edit {} manually", config_path.display());
        }
    }

    Ok(())
}
