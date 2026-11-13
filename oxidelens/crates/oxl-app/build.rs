fn main() {
    tauri_build::build();
    test_manifest();
}

/// tauri-build embeds the app manifest, which asks Windows for Common
/// Controls v6, in the app's binary only. A test that drives commands
/// through Tauri's IPC (`tests/contract.rs`) links code importing
/// `TaskDialogIndirect`, which only v6 has: without the manifest the test
/// binary cannot even start (`STATUS_ENTRYPOINT_NOT_FOUND`). So the test
/// binaries get the same dependency, through the MSVC linker.
fn test_manifest() {
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !msvc {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let manifest = out.join("test-common-controls.manifest");
    std::fs::write(&manifest, COMMON_CONTROLS_V6).expect("OUT_DIR is writable");
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

/// tauri-build's default app manifest (`windows-app-manifest.xml`).
const COMMON_CONTROLS_V6: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
