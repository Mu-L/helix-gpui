use std::sync::{Arc, Mutex};

use anyhow::{Context, Error, Result};
use helix_loader::VERSION_AND_GIT_HASH;
use helix_term::args::Args;
use helix_term::config::{Config, ConfigLoadError};
use helix_view::Editor;

use gpui::{
    actions, App, AppContext, Context as _, Menu, MenuItem, TitlebarOptions, VisualContext as _,
    WindowBackgroundAppearance, WindowKind, WindowOptions,
};

use application::Application;

mod application;
mod document;
mod info_box;
mod notification;
mod picker;
mod prompt;
mod statusline;
mod utils;
mod workspace;

#[derive(Clone)]
pub struct EditorModel {
    inner: Arc<Mutex<Editor>>,
}

impl EditorModel {
    pub fn lock(&self) -> std::sync::MutexGuard<Editor> {
        self.inner.lock().unwrap()
    }

    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<Editor>> {
        self.inner.try_lock().ok()
    }
}

fn setup_logging(verbosity: u64) -> Result<()> {
    let mut base_config = fern::Dispatch::new();

    base_config = match verbosity {
        0 => base_config.level(log::LevelFilter::Warn),
        1 => base_config.level(log::LevelFilter::Info),
        2 => base_config.level(log::LevelFilter::Debug),
        _3_or_more => base_config.level(log::LevelFilter::Trace),
    };

    // Separate file config so we can include year, month and day in file logs
    let file_config = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {} [{}] {}",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"),
                record.target(),
                record.level(),
                message
            ))
        })
        .chain(std::io::stdout())
        .chain(fern::log_file(helix_loader::log_file())?);

    base_config.chain(file_config).apply()?;

    Ok(())
}

fn main() -> Result<()> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle();
    let _guard = handle.enter();
    let app = init_editor().unwrap().unwrap();
    drop(_guard);
    gui_main(app, handle.clone());
    Ok(())
}

fn window_options(_cx: &mut AppContext) -> gpui::WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: None, //Some(point(px(9.0), px(9.0))),
        }),
        bounds: None,
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        display_id: None,
        fullscreen: false,
        window_background: WindowBackgroundAppearance::Opaque,
    }
}

actions!(workspace, [About, Quit, OpenFile, ShowModal]);

fn app_menus() -> Vec<Menu<'static>> {
    vec![
        Menu {
            name: "Helix",
            items: vec![
                MenuItem::action("About", About),
                MenuItem::separator(),
                // MenuItem::action("Settings", OpenSettings),
                // MenuItem::separator(),
                // MenuItem::action("Hide Helix", Hide),
                // MenuItem::action("Hide Others", HideOthers),
                // MenuItem::action("Show All", ShowAll),
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File",
            items: vec![
                MenuItem::action("Open File", OpenFile),
                // MenuItem::action("Open Directory", OpenDirectory),
            ],
        },
    ]
}

#[derive(Debug)]
pub enum Update {
    Redraw,
    Prompt(prompt::Prompt),
    Picker(picker::Picker),
    Info(helix_view::info::Info),
    EditorEvent(helix_view::editor::EditorEvent),
}

impl gpui::EventEmitter<Update> for EditorModel {}

fn gui_main(app: Application, handle: tokio::runtime::Handle) {
    App::new().run(|cx: &mut AppContext| {
        let options = window_options(cx);

        cx.open_window(options, |cx| {
            let editor = EditorModel {
                inner: Arc::new(Mutex::new(app.editor)),
            };
            let editor_1 = editor.clone();

            let editor = cx.new_model(|_mc| editor);
            let editor_model = editor.clone();
            let handle_1 = handle.clone();

            cx.spawn(move |mut cx| {
                let handle_1 = handle_1.clone();
                let model = editor_model.clone();

                async move {
                    use std::time::Duration;
                    let model = model.clone();
                    let editor = editor_1.clone();
                    let _guard = handle_1.enter();
                    loop {
                        use futures_util::future::FutureExt;
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        if let Some(mut editor) = editor.try_lock() {
                            if let Some(event) = editor.wait_event().now_or_never() {
                                drop(editor);
                                let _ = cx.update_model(&model, |_, cx| {
                                    cx.emit(Update::EditorEvent(event));
                                });
                            }
                        }
                    }
                }
            })
            .detach();
            let view = cx.new_model(|_mc| app.view);
            let compositor = cx.new_model(|_mc| app.compositor);

            cx.activate(true);
            cx.set_menus(app_menus());

            cx.new_view(|cx| {
                cx.subscribe(&editor, |w: &mut workspace::Workspace, _, ev, cx| {
                    w.handle_event(ev, cx);
                })
                .detach();
                workspace::Workspace::new(editor, view, compositor, handle, cx)
            })
        });
    })
}

fn init_editor() -> Result<Option<Application>> {
    let help = format!(
        "\
{} {}
{}
{}

USAGE:
    hx [FLAGS] [files]...

ARGS:
    <files>...    Sets the input file to use, position can also be specified via file[:row[:col]]

FLAGS:
    -h, --help                     Prints help information
    --tutor                        Loads the tutorial
    --health [CATEGORY]            Checks for potential errors in editor setup
                                   CATEGORY can be a language or one of 'clipboard', 'languages'
                                   or 'all'. 'all' is the default if not specified.
    -g, --grammar {{fetch|build}}    Fetches or builds tree-sitter grammars listed in languages.toml
    -c, --config <file>            Specifies a file to use for configuration
    -v                             Increases logging verbosity each use for up to 3 times
    --log <file>                   Specifies a file to use for logging
                                   (default file: {})
    -V, --version                  Prints version information
    --vsplit                       Splits all given files vertically into different windows
    --hsplit                       Splits all given files horizontally into different windows
    -w, --working-dir <path>       Specify an initial working directory
    +N                             Open the first given file at line number N
",
        env!("CARGO_PKG_NAME"),
        VERSION_AND_GIT_HASH,
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_DESCRIPTION"),
        helix_loader::default_log_file().display(),
    );

    let mut args = Args::parse_args().context("could not parse arguments")?;

    helix_loader::initialize_config_file(args.config_file.clone());
    helix_loader::initialize_log_file(args.log_file.clone());

    // Help has a higher priority and should be handled separately.
    if args.display_help {
        print!("{}", help);
        std::process::exit(0);
    }

    if args.display_version {
        println!("helix {}", VERSION_AND_GIT_HASH);
        std::process::exit(0);
    }

    if args.health {
        if let Err(err) = helix_term::health::print_health(args.health_arg) {
            // Piping to for example `head -10` requires special handling:
            // https://stackoverflow.com/a/65760807/7115678
            if err.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(err.into());
            }
        }

        std::process::exit(0);
    }

    if args.fetch_grammars {
        helix_loader::grammar::fetch_grammars()?;
        return Ok(None);
    }

    if args.build_grammars {
        helix_loader::grammar::build_grammars(None)?;
        return Ok(None);
    }

    setup_logging(args.verbosity).context("failed to initialize logging")?;

    // Before setting the working directory, resolve all the paths in args.files
    for (path, _) in args.files.iter_mut() {
        *path = helix_stdx::path::canonicalize(&path);
    }

    // NOTE: Set the working directory early so the correct configuration is loaded. Be aware that
    // Application::new() depends on this logic so it must be updated if this changes.
    if let Some(path) = &args.working_directory {
        helix_stdx::env::set_current_working_dir(path)?;
    } else if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        // If the first file is a directory, it will be the working directory unless -w was specified
        helix_stdx::env::set_current_working_dir(path)?;
    }

    let config = match Config::load_default() {
        Ok(config) => config,
        Err(ConfigLoadError::Error(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Config::default()
        }
        Err(ConfigLoadError::Error(err)) => return Err(Error::new(err)),
        Err(ConfigLoadError::BadConfig(err)) => {
            eprintln!("Bad config: {}", err);
            eprintln!("Press <ENTER> to continue with default config");
            use std::io::Read;
            let _ = std::io::stdin().read(&mut []);
            Config::default()
        }
    };

    let lang_loader = helix_core::config::user_lang_loader().unwrap_or_else(|err| {
        eprintln!("{}", err);
        eprintln!("Press <ENTER> to continue with default language config");
        use std::io::Read;
        // This waits for an enter press.
        let _ = std::io::stdin().read(&mut []);
        helix_core::config::default_lang_loader()
    });

    // TODO: use the thread local executor to spawn the application task separately from the work pool
    let app = application::init_editor(args, config, lang_loader)
        .context("unable to create new application")?;

    Ok(Some(app))
}
