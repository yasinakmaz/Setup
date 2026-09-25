//! AppImage packaging for Linux installers.
//!
//! Layout of the produced file:
//!
//! ```text
//! [AppImage type 2 runtime (ELF)][squashfs: AppRun, .desktop, icon][installer payload]
//! ```
//!
//! `AppRun` is the installer runtime *without* payload. The payload is not
//! put inside the squashfs (it is already compressed, and FUSE reads would
//! slow extraction down); it is appended to the AppImage file itself, which
//! the runtime finds through the `$APPIMAGE` variable set by the AppImage
//! runtime. The AppImage is only the installer's container: the installed
//! application is a normal user-local installation.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use inst_fetch::{Cache, ContentHash};
use inst_model::Arch;

pub struct AppImageInput<'a> {
    pub runtime_exe: &'a Path,
    pub payload: &'a Path,
    pub output: &'a Path,
    pub product_id: &'a str,
    pub name: &'a str,
    pub icon_png: Option<&'a [u8]>,
    pub arch: Arch,
    pub cache: &'a Cache,
    pub work_dir: &'a Path,
}

const RUNTIME_BASE: &str = "https://github.com/AppImage/type2-runtime/releases/download/continuous";

/// A generic 64x64 installer icon (PNG) used when the project has none.
fn default_icon() -> Vec<u8> {
    // 1x1 transparent PNG, scaled by desktops; avoids a hard dependency on
    // an image encoder in the build system.
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

/// Obtains the AppImage type 2 runtime: `$INST_APPIMAGE_RUNTIME` (offline
/// builds) or the official AppImage project release. The downloaded file is
/// pinned by hash in the Studio cache so later builds use identical bytes.
fn appimage_runtime(input: &AppImageInput<'_>) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("INST_APPIMAGE_RUNTIME") {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("{} not found", p.display()))
        };
    }
    let arch = match input.arch {
        Arch::X64 => "x86_64",
        Arch::Arm64 => "aarch64",
    };
    let pin_file = input.work_dir.join(format!("appimage-runtime-{arch}.pin"));
    if let Some((hash, size)) = fs::read_to_string(&pin_file).ok().and_then(|s| {
        let (h, n) = s.trim().split_once(' ')?;
        Some((ContentHash::parse(h).ok()?, n.parse::<u64>().ok()?))
    }) && let Ok(Some(path)) = input.cache.get_verified(&hash, size)
    {
        return Ok(path);
    }
    let url = format!("{RUNTIME_BASE}/runtime-{arch}");
    let transport =
        inst_fetch::transport::UreqTransport::new(&inst_fetch::TransportConfig::default())
            .map_err(|e| e.to_string())?;
    let resp = inst_fetch::Transport::get(&transport, &url, 0, None).map_err(|e| e.to_string())?;
    if resp.status != 200 {
        return Err(format!(
            "HTTP {} downloading the AppImage runtime",
            resp.status
        ));
    }
    let tmp = input
        .work_dir
        .join(format!("appimage-runtime-{arch}.download"));
    let mut out = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut hasher = ContentHash::Sha256([0; 32]).hasher();
    let mut body = resp.body;
    let mut buf = vec![0u8; 64 << 10];
    let mut size = 0u64;
    loop {
        let n = io::Read::read(&mut body, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        size += n as u64;
    }
    drop(out);
    let hash = hasher.finalize();
    let bytes = fs::read(&tmp).map_err(|e| e.to_string())?;
    if bytes.len() < 4 || &bytes[..4] != b"\x7fELF" {
        return Err("downloaded AppImage runtime is not an ELF executable".into());
    }
    let info = inst_fetch::ItemInfo {
        vendor: "AppImage project".into(),
        product: "type2-runtime".into(),
        version: "continuous".into(),
        arch: arch.into(),
        platform: "linux".into(),
        file_name: format!("runtime-{arch}"),
    };
    let path = input
        .cache
        .insert_file(&tmp, &hash, &info)
        .map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&tmp);
    fs::write(&pin_file, format!("{hash} {size}\n")).map_err(|e| e.to_string())?;
    Ok(path)
}

fn desktop_entry(name: &str, id: &str) -> String {
    let safe = name.replace(['\n', '\r'], " ");
    format!(
        "[Desktop Entry]\nType=Application\nName={safe} Setup\nExec=AppRun\nIcon={id}\nTerminal=false\nCategories=Utility;\nX-AppImage-Integrate=false\n"
    )
}

pub fn package(input: &AppImageInput<'_>) -> Result<(), String> {
    use backhand::compression::Compressor;
    use backhand::{FilesystemCompressor, FilesystemWriter, NodeHeader};

    let runtime = appimage_runtime(input)?;
    let mut fs_writer = FilesystemWriter::default();
    fs_writer.set_compressor(
        FilesystemCompressor::new(Compressor::Gzip, None).map_err(|e| e.to_string())?,
    );
    fs_writer.set_time(0); // reproducible
    fs_writer.set_root_mode(0o755);
    let exec = NodeHeader::new(0o755, 0, 0, 0);
    let file = NodeHeader::new(0o644, 0, 0, 0);
    let apprun = fs::read(input.runtime_exe).map_err(|e| e.to_string())?;
    let icon = input.icon_png.map_or_else(default_icon, <[u8]>::to_vec);
    let desktop = desktop_entry(input.name, input.product_id);
    let icon_name = format!("{}.png", input.product_id);
    let desktop_name = format!("{}.desktop", input.product_id);
    fs_writer
        .push_file(io::Cursor::new(apprun), "AppRun", exec)
        .map_err(|e| e.to_string())?;
    fs_writer
        .push_file(io::Cursor::new(desktop.into_bytes()), &desktop_name, file)
        .map_err(|e| e.to_string())?;
    fs_writer
        .push_file(io::Cursor::new(icon.clone()), &icon_name, file)
        .map_err(|e| e.to_string())?;
    fs_writer
        .push_file(io::Cursor::new(icon), ".DirIcon", file)
        .map_err(|e| e.to_string())?;

    let squashfs = input.work_dir.join("appimage.squashfs");
    {
        let f = fs::File::create(&squashfs).map_err(|e| e.to_string())?;
        let mut w = io::BufWriter::new(f);
        fs_writer.write(&mut w).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())?;
    }

    let mut out = fs::File::create(input.output).map_err(|e| e.to_string())?;
    for part in [runtime.as_path(), squashfs.as_path(), input.payload] {
        io::copy(
            &mut fs::File::open(part).map_err(|e| e.to_string())?,
            &mut out,
        )
        .map_err(|e| e.to_string())?;
    }
    out.sync_all().map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&squashfs);
    Ok(())
}
