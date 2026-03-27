fn main() {
    // Only compile Qt helper when the "qt" feature is enabled.
    if std::env::var("CARGO_FEATURE_QT").is_ok() {
        compile_qt_helper();
    }
}

fn compile_qt_helper() {
    let qt_prefix = find_qt_prefix();

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("src/qt_helper.cpp")
        .flag("-std=c++17")
        .flag("-fPIC")
        .flag("-Wno-error=implicit-function-declaration")
        .flag("-Wno-implicit-function-declaration")
        .flag("-Wno-unused-parameter");

    if cfg!(target_os = "macos") {
        // macOS: Qt installed as frameworks (e.g. via Homebrew).
        let framework_path = format!("{}/lib", qt_prefix);
        build
            .flag(&format!("-F{}", framework_path))
            .flag("-framework").flag("QtCore")
            .flag("-framework").flag("QtWidgets")
            .flag("-framework").flag("QtGui");
        // Include paths for framework headers.
        build
            .include(format!("{}/lib/QtCore.framework/Headers", qt_prefix))
            .include(format!("{}/lib/QtWidgets.framework/Headers", qt_prefix))
            .include(format!("{}/lib/QtGui.framework/Headers", qt_prefix))
            .include(format!("{}/include", qt_prefix))
            .include(format!("{}/include/QtCore", qt_prefix))
            .include(format!("{}/include/QtWidgets", qt_prefix))
            .include(format!("{}/include/QtGui", qt_prefix));

        println!("cargo:rustc-link-search=framework={}", framework_path);
        println!("cargo:rustc-link-lib=framework=QtCore");
        println!("cargo:rustc-link-lib=framework=QtWidgets");
        println!("cargo:rustc-link-lib=framework=QtGui");
    } else {
        // Linux: Qt installed as shared libraries, use pkg-config.
        let qt_include = format!("{}/include", qt_prefix);
        build
            .include(&qt_include)
            .include(format!("{}/QtCore", qt_include))
            .include(format!("{}/QtWidgets", qt_include))
            .include(format!("{}/QtGui", qt_include));

        println!("cargo:rustc-link-search={}/lib", qt_prefix);
        println!("cargo:rustc-link-lib=Qt6Core");
        println!("cargo:rustc-link-lib=Qt6Widgets");
        println!("cargo:rustc-link-lib=Qt6Gui");
    }

    build.compile("qt_helper");

    // Also link C++ stdlib.
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=c++");
    } else {
        println!("cargo:rustc-link-lib=stdc++");
    }

    println!("cargo:rerun-if-changed=src/qt_helper.cpp");
}

fn find_qt_prefix() -> String {
    // Try QTDIR env var first.
    if let Ok(dir) = std::env::var("QTDIR") {
        return dir;
    }

    // macOS: try Homebrew.
    if cfg!(target_os = "macos") {
        if let Ok(output) = std::process::Command::new("brew")
            .args(["--prefix", "qt"])
            .output()
        {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).trim().to_owned();
            }
        }
    }

    // Linux: try pkg-config.
    if let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--variable=prefix", "Qt6Core"])
        .output()
    {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().to_owned();
        }
    }

    // Fallback.
    "/usr".to_owned()
}
