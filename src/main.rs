#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    future::IntoFuture,
    process::Stdio,
    sync::OnceLock,
    time::{Duration, Instant},
};

use auto_launch::AutoLaunchBuilder;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Request, WebSocketUpgrade,
    },
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use http::{Method, StatusCode};
use reqwest::Url;
use tao::event_loop::EventLoopBuilder;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};
use tower_http::cors::CorsLayer;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder, TrayIconEvent,
};

async fn adb_start(path: &str) -> tokio::io::Result<()> {
    let mut command = Command::new(path);
    command.args(&["server", "nodaemon"]);
    command.env("ADB_MDNS_OPENSCREEN", "1");
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut process = command.spawn()?;
    tokio::spawn(async move { process.wait().await });
    Ok(())
}

async fn adb_connect() -> tokio::io::Result<TcpStream> {
    TcpStream::connect("127.0.0.1:5037").await
}

async fn adb_connect_retry() -> tokio::io::Result<TcpStream> {
    let mut i = 0;
    loop {
        match adb_connect().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if i == 10 {
                    return Err(err);
                } else {
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}

async fn adb_connect_or_start() -> tokio::io::Result<TcpStream> {
    if let Ok(stream) = adb_connect().await {
        return Ok(stream);
    }

    if adb_start("adb").await.is_ok() {
        return adb_connect_retry().await;
    }

    #[cfg(windows)]
    {
        use tokio::fs::write;

        let tmp_dir = env::temp_dir();
        let adb_path = tmp_dir.join("adb.exe");

        write(&adb_path, include_bytes!("../adb/win/adb.exe"))
            .await
            .unwrap();
        write(
            tmp_dir.join("AdbWinApi.dll"),
            include_bytes!("../adb/win/AdbWinApi.dll"),
        )
        .await
        .unwrap();
        write(
            tmp_dir.join("AdbWinUsbApi.dll"),
            include_bytes!("../adb/win/AdbWinUsbApi.dll"),
        )
        .await
        .unwrap();

        adb_start(adb_path.to_str().unwrap()).await.unwrap();
        adb_connect_retry().await
    }

    #[cfg(target_os = "linux")]
    {
        use tokio::fs::write;

        let tmp_dir = env::temp_dir();
        let adb_path = tmp_dir.join("adb");

        use std::os::unix::fs::PermissionsExt;
        write(&adb_path, include_bytes!("../adb/linux/adb"))
            .await
            .unwrap();
        std::fs::set_permissions(&adb_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        adb_start(adb_path.to_str().unwrap()).await.unwrap();
        adb_connect_retry().await
    }

    #[cfg(target_os = "macos")]
    {
        adb_start(
            env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .join("adb")
                .to_str()
                .unwrap(),
        )
        .await
        .unwrap();
        adb_connect_retry().await
    }
}

fn start_browser() {
    open::that_detached("https://app.tangoapp.dev/?desktop=true").unwrap();
}

async fn handle_websocket(ws: WebSocket) {
    let (mut ws_writer, mut ws_reader) = ws.split();

    let (mut adb_reader, mut adb_writer) = adb_connect_or_start().await.unwrap().into_split();

    tokio::join!(
        async {
            while let Some(Ok(message)) = ws_reader.next().await {
                // Don't merge with `if` above to ignore other message types
                if let Message::Binary(packet) = message {
                    adb_writer.write_all(packet.as_ref()).await.unwrap();
                }
            }
            adb_writer.shutdown().await.unwrap();
        },
        async {
            let mut buf = vec![0; 1024 * 1024];
            loop {
                match adb_reader.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        ws_writer.close().await.unwrap();
                        break;
                    }
                    Ok(n) => ws_writer
                        .send(Message::binary(buf[..n].to_vec()))
                        .await
                        .unwrap(),
                }
            }
        }
    );
}

const ARG_AUTO_RUN: &str = "--auto-run";

#[cfg(debug_assertions)]
const PROXY_HOST: &str = "https://app.tangoapp.dev";
#[cfg(not(debug_assertions))]
const PROXY_HOST: &str = "https://app.tangoapp.dev";

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

#[axum::debug_handler]
async fn proxy_request(request: Request) -> Result<Response, Response> {
    println!("proxy_request: {} {}", request.method(), request.uri());

    let url = Url::options()
        .base_url(Some(&Url::parse(PROXY_HOST).unwrap()))
        .parse(&request.uri().to_string())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Bad Request").into_response())?;

    let mut headers = request.headers().clone();
    headers.insert("Host", url.host_str().unwrap().parse().unwrap());

    let (client, request) = CLIENT
        .get_or_init(|| reqwest::Client::new())
        .request(request.method().clone(), url)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(
            request.into_body().into_data_stream(),
        ))
        .build_split();

    let request = request.map_err(|_| (StatusCode::BAD_REQUEST, "Bad Request").into_response())?;

    let response = client
        .execute(request)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Bad Gateway").into_response())?;

    Ok((
        response.status(),
        response.headers().clone(),
        axum::body::Body::new(reqwest::Body::from(response)),
    )
        .into_response())
}

static SINGLE_INSTANCE: OnceLock<single_instance::SingleInstance> = OnceLock::new();

#[tokio::main]
async fn main() {
    // macOS app bundle prevents re-launching by default
    #[cfg(not(target_os = "macos"))]
    {
        use single_instance::SingleInstance;

        let single_instance =
            SINGLE_INSTANCE.get_or_init(|| SingleInstance::new("tango-bridge-rs").unwrap());
        println!(
            "single_instance.is_single(): {}",
            single_instance.is_single()
        );
        if !single_instance.is_single() {
            start_browser();
            return;
        }
    }

    tokio::spawn(async { adb_connect_or_start().await.unwrap() });

    let app = Router::new()
        .nest(
            "/bridge",
            Router::new()
                .route("/ping", get(|| async { "OK" }))
                .route(
                    "/",
                    get(|ws: WebSocketUpgrade| async { ws.on_upgrade(handle_websocket) }),
                )
                .route_layer(
                    CorsLayer::new()
                        .allow_methods([Method::GET, Method::POST])
                        .allow_origin(
                            [
                                "http://localhost:3002",
                                "https://app.tangoapp.dev",
                                "https://beta.tangoapp.dev",
                                "https://tunnel.tangoapp.dev",
                            ]
                            .map(|x| x.parse().unwrap()),
                        )
                        .allow_private_network(true),
                ),
        )
        .fallback(proxy_request);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:15037")
        .await
        .unwrap();

    tokio::spawn(axum::serve(listener, app).into_future());

    let menu_open = MenuItem::new("Open", true, None);

    let auto_launch = AutoLaunchBuilder::new()
        .set_app_name("Tango")
        .set_app_path(env::current_exe().unwrap().to_str().unwrap())
        .set_args(&[ARG_AUTO_RUN])
        .set_use_launch_agent(true)
        .build()
        .unwrap();
    let menu_auto_run = CheckMenuItem::new(
        "Run at startup",
        true,
        auto_launch.is_enabled().unwrap(),
        None,
    );

    let menu_quit = MenuItem::new("Quit", true, None);

    let tray_menu = Menu::new();
    tray_menu
        .append_items(&[
            &menu_open,
            &menu_auto_run,
            &PredefinedMenuItem::separator(),
            &menu_quit,
        ])
        .unwrap();

    let menu_receiver = MenuEvent::receiver();
    let tray_receiver = TrayIconEvent::receiver();

    let mut tray_icon = None;

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::new().build();

    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::EventLoopExtMacOS;

        // https://github.com/glfw/glfw/issues/1552
        event_loop.set_activation_policy(tao::platform::macos::ActivationPolicy::Accessory);
    }

    println!("Starting main loop");

    event_loop.run(move |event, _, control_flow| {
        *control_flow =
            tao::event_loop::ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16));

        if let tao::event::Event::Reopen { .. } = event {
            start_browser();
            return;
        }

        if let tao::event::Event::NewEvents(tao::event::StartCause::Init) = event {
            let image = image::load_from_memory_with_format(
                include_bytes!("../tango.png"),
                image::ImageFormat::Png,
            )
            .unwrap()
            .into_rgba8();
            let (width, height) = image.dimensions();
            let rgba = image.into_raw();
            let icon = tray_icon::Icon::from_rgba(rgba, width, height).unwrap();

            tray_icon = Some(
                TrayIconBuilder::new()
                    .with_tooltip("Tango (rs)")
                    .with_icon(icon)
                    .with_menu(Box::new(tray_menu.clone()))
                    .build()
                    .unwrap(),
            );

            #[cfg(target_os = "macos")]
            unsafe {
                use core_foundation::runloop::{CFRunLoopGetMain, CFRunLoopWakeUp};

                let rl = CFRunLoopGetMain();
                CFRunLoopWakeUp(rl);
            }
        }

        if let Ok(event) = menu_receiver.try_recv() {
            if event.id == menu_open.id() {
                start_browser();
                return;
            }

            if event.id == menu_auto_run.id() {
                if auto_launch.is_enabled().unwrap() {
                    auto_launch.disable().unwrap();
                } else {
                    auto_launch.enable().unwrap();
                }
                menu_auto_run.set_checked(auto_launch.is_enabled().unwrap());
                return;
            }

            if event.id == menu_quit.id() {
                tray_icon.take();
                *control_flow = tao::event_loop::ControlFlow::Exit;
                return;
            }
        }

        if let Ok(TrayIconEvent::Click {
            button: tray_icon::MouseButton::Left,
            button_state: tray_icon::MouseButtonState::Down,
            ..
        }) = tray_receiver.try_recv()
        {
            start_browser();
            return;
        }
    });
}
