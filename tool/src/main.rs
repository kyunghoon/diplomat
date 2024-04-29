use clap::Parser;
use diplomat_tool::ApiInfo;
use std::path::PathBuf;

/// diplomat-tool CLI options, as parsed by [clap-derive].
#[derive(Debug, Parser)]
#[clap(
    name = "diplomat-tool",
    about = "Generate bindings to a target language"
)]
struct Opt {
    /// The target language, "js", "c", "cpp" or "dotnet" (C#).
    #[clap()]
    target_language: String,

    /// The folder that stores the bindings.
    #[clap(value_parser)]
    out_folder: PathBuf,

    #[clap(short, long, value_parser)]
    docs: Option<PathBuf>,

    #[clap(short = 'u', long)]
    docs_base_urls: Vec<String>,

    /// The path to the lib.rs file.
    #[clap(short, long, value_parser, default_value = "src/lib.rs")]
    entry: PathBuf,

    /// The path to an optional config file to override code generation defaults.
    /// This is currently used by the cpp generator to allow for code to be
    /// different libraries.
    #[clap(short, long, value_parser)]
    library_config: Option<PathBuf>,

    #[clap(short = 's', long)]
    silent: bool,

    #[clap()]
    apiname: Option<String>,

    #[clap()]
    refresh_api_fn: Option<String>,

    #[clap()]
    get_api_fn: Option<String>,

    #[clap()]
    additional_includes: Option<Vec<String>>,
}

fn main() -> std::io::Result<()> {
    let opt = Opt::parse();

    let additional_includes = opt.additional_includes.as_ref().map(|v| v.iter().map(|i| i.as_str()).collect::<Vec<_>>());

    let api_info = match opt.apiname.as_ref().zip(opt.refresh_api_fn.as_ref()).zip(opt.get_api_fn.as_ref()).zip(additional_includes.as_ref()) {
        None => None,
        Some((((apiname, refresh_api_fn), get_api_fn), additional_includes)) => {
            Some(ApiInfo {
                apiname: apiname,
                refresh_api_fn: refresh_api_fn,
                get_api_fn: get_api_fn,
                additional_includes: additional_includes.as_ref(),
            })
        }
    };

    diplomat_tool::gen(
        &opt.entry,
        &opt.target_language,
        &opt.out_folder,
        opt.docs.as_deref(),
        &diplomat_core::ast::DocsUrlGenerator::with_base_urls(
            opt.docs_base_urls
                .iter()
                .filter_map(|entry| entry.strip_prefix("*:").map(ToString::to_string))
                .next(),
            opt.docs_base_urls
                .iter()
                .filter(|entry| !entry.starts_with('*'))
                .map(|entry| {
                    let mut parts = entry.splitn(2, ':');
                    (
                        parts.next().unwrap().to_string(),
                        parts
                            .next()
                            .expect("Expected syntax <crate>|*:<url>")
                            .to_string(),
                    )
                })
                .collect(),
        ),
        opt.library_config.as_deref(),
        opt.silent,
        None,
        api_info,
    )
}
