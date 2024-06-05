#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    convert::Infallible,
    env,
    process::Stdio,
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use auto_launch::AutoLaunchBuilder;
use futures_util::{SinkExt, Stream, StreamExt};
use tao::event_loop::EventLoopBuilder;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder, TrayIconEvent,
};
use warp::{filters::ws::Message, reply::Response, Filter};

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

async fn handle_websocket(ws: warp::ws::WebSocket) {
    let (mut ws_writer, mut ws_reader) = ws.split();

    let (mut adb_reader, mut adb_writer) = adb_connect_or_start().await.unwrap().into_split();

    tokio::join!(
        async {
            while let Some(Ok(message)) = ws_reader.next().await {
                if message.is_binary() {
                    adb_writer.write_all(message.as_bytes()).await.unwrap();
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
const PROXY_HOST: &str = "https://tunnel.tangoapp.dev";
#[cfg(not(debug_assertions))]
const PROXY_HOST: &str = "https://app.tangoapp.dev";

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

async fn proxy_request(
    method: warp::http::Method,
    tail: warp::path::Tail,
    query: String,
    body: impl Stream<Item = Result<impl warp::Buf, warp::Error>> + Send + Sync + 'static,
) -> Result<Response, warp::Rejection> {
    println!("proxy_request: {} {}", method, tail.as_str());

    let method = match method {
        warp::http::Method::GET => reqwest::Method::GET,
        warp::http::Method::POST => reqwest::Method::POST,
        warp::http::Method::PUT => reqwest::Method::PUT,
        warp::http::Method::DELETE => reqwest::Method::DELETE,
        warp::http::Method::HEAD => reqwest::Method::HEAD,
        warp::http::Method::OPTIONS => reqwest::Method::OPTIONS,
        warp::http::Method::PATCH => reqwest::Method::PATCH,
        warp::http::Method::TRACE => reqwest::Method::TRACE,
        _ => return Err(warp::reject::not_found()),
    };

    let url = if query.is_empty() {
        format!("{}/{}", PROXY_HOST, tail.as_str())
    } else {
        format!("{}/{}?{}", PROXY_HOST, tail.as_str(), query)
    };
    let (client, request) = CLIENT
        .get_or_init(|| reqwest::Client::new())
        .request(method, &url)
        .body(reqwest::Body::wrap_stream(body.map(|result| {
            result.map(|mut buf| {
                let remaining = buf.remaining();
                buf.copy_to_bytes(remaining)
            })
        })))
        .build_split();
    let request = request.map_err(|err| {
        println!("build request error: {}", err);
        warp::reject::not_found()
    })?;
    let response = client.execute(request).await.map_err(|err| {
        println!("send request error: {}", err);
        warp::reject::not_found()
    })?;

    Ok({
        let builder = warp::hyper::Response::builder().status(response.status().as_u16());
        let builder = response
            .headers()
            .iter()
            .fold(builder, |builder, (key, value)| {
                builder.header(key.as_str(), value.to_str().unwrap())
            });
        builder
            .body(warp::hyper::Body::from(response.bytes().await.unwrap()))
            .unwrap()
    })
}

static SINGLE_INSTANCE: OnceLock<single_instance::SingleInstance> = OnceLock::new();

fn main() {
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

    thread::spawn(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(10)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::spawn(async { adb_connect_or_start().await.unwrap() });

                let ping = warp::get()
                    .and(warp::path("ping"))
                    .and(warp::path::end())
                    .map(warp::reply)
                    .with(
                        warp::cors()
                            .allow_origins([
                                "http://localhost:3002",
                                "https://app.tangoapp.dev",
                                "https://beta.tangoapp.dev",
                            ])
                            .build(),
                    );
                let ws = warp::path::end()
                    .and(warp::ws())
                    .map(|ws: warp::ws::Ws| ws.on_upgrade(|ws| handle_websocket(ws)));
                let bridge = warp::path("bridge")
                    .and(ping.or(ws))
                    .with(warp::trace::request());

                let proxy = warp::any()
                    .and(warp::method())
                    .and(warp::path::tail())
                    .and(
                        warp::query::raw()
                            .or_else(|_| async { Ok::<(String,), Infallible>(("".to_owned(),)) }),
                    )
                    .and(warp::body::stream())
                    .and_then(proxy_request)
                    .with(warp::trace::request());

                let serve = warp::serve(bridge.or(proxy)).bind(([127, 0, 0, 1], 15037));

                if env::args().all(|arg| arg != ARG_AUTO_RUN) {
                    start_browser()
                }

                serve.await;
            })
    });

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
