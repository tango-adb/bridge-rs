rustup target add x86_64-pc-windows-msvc
cargo build --target x86_64-pc-windows-msvc --release
mkdir -Force bin\x86-64
copy target\x86_64-pc-windows-msvc\release\tango_bridge_rs.exe bin\x86-64\tango-bridge.exe

rustup target add i686-pc-windows-msvc
cargo build --target i686-pc-windows-msvc --release
mkdir -Force bin\ia-32
copy target\i686-pc-windows-msvc\release\tango_bridge_rs.exe bin\ia-32\tango-bridge.exe
