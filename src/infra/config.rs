use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::{AppContext, InitArgs};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config
{
    /// Default ignore patterns (in addition to .gitignore)
    pub ignore_patterns: Vec<String>,

    /// Default output directory
    pub output_dir: Option<PathBuf>,

    /// Default extraction settings
    pub extract: ExtractConfig,

    /// Default tree settings  
    pub tree: TreeConfig,

    /// Default symbol extraction settings
    pub symbols: SymbolsConfig,

    /// Default chunking settings
    pub chunk: ChunkConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractConfig
{
    pub annotate: bool,
    pub fence: bool,
    pub output_file: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TreeConfig
{
    pub max_depth: Option<usize>,
    pub show_hidden: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SymbolsConfig
{
    pub languages: Vec<String>,
    pub include_private: bool,
    pub output_file: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkConfig
{
    pub max_tokens: usize,
    pub model: String,
    pub output_dir: String,
}

impl Default for Config
{
    fn default() -> Self
    {
        Self {
            ignore_patterns: vec![
                "target/".to_string(),
                "node_modules/".to_string(),
                "dist/".to_string(),
                "build/".to_string(),
                ".git/".to_string(),
                "*.pyc".to_string(),
                "__pycache__/".to_string(),
                ".DS_Store".to_string(),
                "Thumbs.db".to_string(),
            ],
            output_dir: None,
            extract: ExtractConfig {
                annotate: false,
                fence: false,
                output_file: "extracted_source.txt".to_string(),
            },
            tree: TreeConfig { max_depth: None, show_hidden: false },
            symbols: SymbolsConfig {
                languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
                include_private: false,
                output_file: ".rup/symbols.jsonl".to_string(),
            },
            chunk: ChunkConfig {
                max_tokens: 4000,
                model: "gpt-4".to_string(),
                output_dir: "chunks".to_string(),
            },
        }
    }
}

pub fn load_config() -> Result<Config>
{
    let mut builder = config::Config::builder();

    // Load from config files in priority order
    let config_paths = ["roughup.toml", "roughup.yaml", "roughup.json", ".roughup.toml"];

    for path in &config_paths
    {
        if Path::new(path).exists()
        {
            builder = builder.add_source(config::File::with_name(path));
            break;
        }
    }

    // Add environment variables with ROUGHUP_ prefix
    builder = builder.add_source(config::Environment::with_prefix("ROUGHUP").separator("_"));

    let cfg = builder
        .build()
        .context("Failed to load configuration")?;
    let parsed: Config = cfg
        .try_deserialize()
        .context("Failed to parse configuration")?;

    Ok(parsed)
}

pub fn init(
    args: InitArgs,
    ctx: &AppContext,
) -> Result<()>
{
    let config_path = args
        .path
        .join("roughup.toml");

    if config_path.exists() && !args.force
    {
        anyhow::bail!(
            "Config file already exists at {}. Use --force to overwrite.",
            config_path.display()
        );
    }

    let config = Config::default();
    let toml_string =
        toml::to_string_pretty(&config).context("Failed to serialize default config")?;

    std::fs::write(&config_path, toml_string).context("Failed to write config file")?;

    if !ctx.quiet
    {
        println!("Created config file at {}", config_path.display());
    }
    Ok(())
}
