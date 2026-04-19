use std::net::SocketAddr;
use std::path::PathBuf;
use axum::Router;
use tower_http::{cors::CorsLayer, services::ServeDir};

fn usage() -> ! {
    eprintln!("usage: camdl serve <output-dir> [--port PORT]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --port N   port to listen on (default: 4280)");
    eprintln!();
    eprintln!("Serves a results/ (or equivalent output) directory as static HTTP files with CORS enabled.");
    eprintln!("  GET /sims/manifest.json                                 → batch manifest (if present)");
    eprintln!("  GET /sims/<stem>-<sim8>/<scen_slug>-<scen8>/seed_<N>/   → simulate run + run.json");
    eprintln!("  GET /fits/<stem>-<fit8>/                                → fit root + run.json");
    eprintln!("  GET /geo/<file>                                         → GeoJSON boundary files (if present)");
    std::process::exit(1);
}

pub fn cmd_serve(args: &[String]) {
    let mut output_dir: Option<PathBuf> = None;
    let mut port: u16 = 4280;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                if i >= args.len() { eprintln!("--port requires a value"); std::process::exit(1); }
                port = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("--port must be a number between 1 and 65535");
                    std::process::exit(1);
                });
            }
            "--help" | "-h" => usage(),
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                usage();
            }
            path => {
                if output_dir.is_some() {
                    eprintln!("unexpected argument: {}", path);
                    usage();
                }
                output_dir = Some(PathBuf::from(path));
            }
        }
        i += 1;
    }

    let output_dir = output_dir.unwrap_or_else(|| usage());

    if !output_dir.exists() {
        eprintln!("error: output directory does not exist: {}", output_dir.display());
        std::process::exit(1);
    }
    if !output_dir.is_dir() {
        eprintln!("error: not a directory: {}", output_dir.display());
        std::process::exit(1);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| { eprintln!("tokio runtime error: {}", e); std::process::exit(1); });

    rt.block_on(async move {
        let cors = CorsLayer::permissive();
        let app = Router::new()
            .nest_service("/", ServeDir::new(&output_dir))
            .layer(cors);

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr).await
            .unwrap_or_else(|e| {
                eprintln!("error: could not bind to port {}: {}", port, e);
                std::process::exit(1);
            });

        eprintln!("camdl serve: http://127.0.0.1:{}", port);
        eprintln!("  output dir: {}", output_dir.display());
        eprintln!("  press Ctrl+C to stop");

        axum::serve(listener, app).await
            .unwrap_or_else(|e| { eprintln!("server error: {}", e); std::process::exit(1); });
    });
}
