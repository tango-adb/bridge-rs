use std::{env, mem, path::Path, process::Stdio, time::Duration};

use tokio::{net::TcpStream, process::Command};

async fn adb_start(path: &Path) -> tokio::io::Result<()> {
    let mut command = Command::new(path);
    command.args(&["server", "nodaemon"]);

    if path.is_absolute() {
        command.current_dir(path.parent().unwrap());
    }

    // Enable built-in mDNS service
    command.env("ADB_MDNS_OPENSCREEN", "1");
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    #[cfg(unix)]
    command.process_group(0);

    #[cfg(windows)]
    // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | DETACHED_PROCESS
    command.creation_flags(0x00000200 | 0x08000000 | 0x00000008);

    // Tokio documentation recommends not dropping the `Child`
    // to make sure the child process exits correctly on Unix platforms
    mem::forget(command.spawn()?);

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

pub async fn connect_or_start() -> tokio::io::Result<TcpStream> {
    if let Ok(stream) = adb_connect().await {
        return Ok(stream);
    }

    // Try system installed adb first
    if adb_start(Path::new("adb")).await.is_ok() {
        return adb_connect_retry().await;
    }

    #[cfg(windows)]
    {
        use std::fs::exists;
        use tokio::fs::write;

        let tmp_dir = env::temp_dir();
        let adb_path = tmp_dir.join("adb.exe");

        if !exists(&adb_path)? {
            tokio::try_join!(
                write(&adb_path, include_bytes!("../adb/win/adb.exe")),
                write(
                    tmp_dir.join("AdbWinApi.dll"),
                    include_bytes!("../adb/win/AdbWinApi.dll"),
                ),
                write(
                    tmp_dir.join("AdbWinUsbApi.dll"),
                    include_bytes!("../adb/win/AdbWinUsbApi.dll"),
                )
            )?;
        }

        adb_start(&adb_path).await?;
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs::exists;
        use std::os::unix::fs::PermissionsExt;
        use tokio::fs::write;

        let tmp_dir = env::temp_dir();
        let adb_path = tmp_dir.join("adb");

        if !exists(&adb_path)? {
            write(&adb_path, include_bytes!("../adb/linux/adb")).await?;
            std::fs::set_permissions(&adb_path, std::fs::Permissions::from_mode(0o755))?;
        }

        adb_start(&adb_path).await?;
    }

    #[cfg(target_os = "macos")]
    {
        adb_start(env::current_exe().unwrap().parent().unwrap().join("adb").as_path()).await?;
    }

    adb_connect_retry().await
}
