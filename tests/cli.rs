//! End-to-end: render a recording with the real binary.

use std::path::PathBuf;
use std::process::Command;

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("term2gif-test-{}-{name}", std::process::id()))
}

#[test]
fn renders_a_recording() {
    let cast = temp_path("in.cast");
    let gif = temp_path("out.gif");
    std::fs::write(
        &cast,
        "{\"version\":2,\"width\":40,\"height\":5}\n\
         [0.1,\"o\",\"\\u001b[1;32mhello\\u001b[0m \u{65e5}\u{672c} \u{1f642}\\r\\n\"]\n\
         [0.5,\"o\",\"\u{250c}\u{2500}\u{2510} \u{2588}\u{2592}\\r\\n\"]\n\
         [0.8,\"r\",\"50x6\"]\n\
         [1.0,\"o\",\"done\\r\\n\"]\n",
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_term2gif"))
        .arg(&gif)
        .arg("--from-cast")
        .arg(&cast)
        .args(["--no-font-detect", "-q"])
        .status()
        .unwrap();
    let data = std::fs::read(&gif).unwrap();
    let _ = std::fs::remove_file(&cast);
    let _ = std::fs::remove_file(&gif);

    assert!(status.success());
    assert_eq!(&data[..6], b"GIF89a");
    // 50x6 cells (the largest size) of the built-in 14px font: 8x20 pixels,
    // plus one cell width of padding on each side.
    let width = u16::from_le_bytes([data[6], data[7]]);
    let height = u16::from_le_bytes([data[8], data[9]]);
    assert_eq!((width, height), (50 * 8 + 16, 6 * 20 + 16));
}

#[test]
fn rejects_a_missing_recording() {
    let gif = temp_path("missing.gif");
    let output = Command::new(env!("CARGO_BIN_EXE_term2gif"))
        .arg(&gif)
        .args(["--from-cast", "does-not-exist.cast", "--no-font-detect"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot open"));
    assert!(!gif.exists());
}
