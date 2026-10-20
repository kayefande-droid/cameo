use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct ToolStatus {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub version: String,
    pub detail: String,
    pub exe: String,
}

/// Run `<exe> <args>` via cmd.exe, returning the first non-empty output line.
fn probe(exe: &str, args: &str) -> Result<String, String> {
    let output = std::process::Command::new("cmd")
        .args(["/C", &format!("{exe} {args}")])
        .output()
        .map_err(|e| e.to_string())?;
    let mut line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    if line.is_empty() {
        line = String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string();
    }
    if output.status.success() || !line.is_empty() {
        Ok(line)
    } else {
        Err(format!(
            "exit code {}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

fn try_probe(id: &str, name: &str, exe: &str, args: &str) -> ToolStatus {
    match probe(exe, args) {
        Ok(first_line) => ToolStatus {
            id: id.into(),
            name: name.into(),
            available: true,
            version: first_line,
            detail: format!("detected via PATH lookup of '{exe}'"),
            exe: exe.into(),
        },
        Err(e) => ToolStatus {
            id: id.into(),
            name: name.into(),
            available: false,
            version: String::new(),
            detail: e,
            exe: exe.into(),
        },
    }
}

#[tauri::command]
pub async fn detect_dev_tools() -> Result<Vec<ToolStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let tools: Vec<(&str, &str, &str, &str)> = vec![
            ("git", "Git", "git", "--version"),
            ("code", "VS Code", "code", "--version"),
            ("cursor", "Cursor", "cursor", "--version"),
            ("node", "Node.js", "node", "--version"),
            ("npm", "npm", "npm", "--version"),
            ("pnpm", "pnpm", "pnpm", "--version"),
            ("cargo", "Cargo (Rust)", "cargo", "--version"),
            ("rustc", "rustc", "rustc", "--version"),
            ("dotnet", ".NET SDK", "dotnet", "--version"),
            ("python", "Python", "python", "--version"),
            ("gcc", "GCC", "gcc", "--version"),
            ("clang", "Clang", "clang", "--version"),
            ("cmake", "CMake", "cmake", "--version"),
            ("docker", "Docker", "docker", "--version"),
            ("php", "PHP", "php", "--version"),
            ("composer", "Composer", "composer", "--version"),
            ("java", "JDK", "java", "-version"),
            ("mvn", "Maven", "mvn", "--version"),
            ("gradle", "Gradle", "gradle", "--version"),
            ("flutter", "Flutter", "flutter", "--version"),
            ("dart", "Dart", "dart", "--version"),
        ];
        let mut handles = Vec::new();
        for (id, name, exe, args) in tools {
            handles.push(std::thread::spawn(move || try_probe(id, name, exe, args)));
        }
        let mut out: Vec<ToolStatus> = handles
            .into_iter()
            .map(|h| h.join().unwrap_or(ToolStatus {
                id: "?".into(),
                name: "probe crashed".into(),
                available: false,
                version: String::new(),
                detail: "thread panicked".into(),
                exe: String::new(),
            }))
            .collect();

        // vswere probe for the MSVC/Visual Studio toolchain.
        let vs = try_probe(
            "msvc",
            "Visual Studio build tools",
            "vswhere",
            "-latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath",
        );
        if vs.available {
            out.insert(0, vs);
        } else {
            let mut vs_missing = vs;
            vs_missing.detail = format!(
                "{} (no MSVC C++ workload found via vswhere)",
                vs_missing.detail
            );
            out.push(vs_missing);
        }

        // Android SDK is an environment check rather than a versioned executable.
        let home = std::env::var("ANDROID_HOME").unwrap_or_default();
        let alt = std::env::var("ANDROID_SDK_ROOT").unwrap_or_default();
        let path = if home.is_empty() { alt.clone() } else { home };
        out.push(ToolStatus {
            id: "android-sdk".into(),
            name: "Android SDK".into(),
            available: !path.is_empty(),
            version: String::new(),
            detail: if path.is_empty() {
                "ANDROID_HOME is not set".into()
            } else {
                format!("ANDROID_HOME is set to {path}")
            },
            exe: String::new(),
        });

        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
pub struct ProjectInspection {
    pub path: String,
    pub kind: String,
    pub frameworks: Vec<String>,
    pub scripts: Vec<(String, String)>,
    pub top_level: Vec<String>,
    pub key_files: Vec<(String, String)>,
}

/// Build a compact description of a project directory for ATLAS analysis.
#[tauri::command]
pub async fn inspect_project(dir: String) -> Result<ProjectInspection, String> {
    if dir.trim().is_empty() {
        return Err("no directory selected".into());
    }
    let path = std::path::PathBuf::from(dir.trim());
    if !path.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    tauri::async_runtime::spawn_blocking(move || inspect_sync(&path))
        .await
        .map_err(|e| e.to_string())?
}

fn inspect_sync(dir: &std::path::Path) -> Result<ProjectInspection, String> {
    let mut frameworks: Vec<String> = Vec::new();
    let mut scripts: Vec<(String, String)> = Vec::new();
    let mut top_level: Vec<String> = Vec::new();
    let mut key_files: Vec<(String, String)> = Vec::new();

    let read_snippet = |f: &std::path::Path, n: usize| -> String {
        std::fs::read_to_string(f)
            .map(|s| s.lines().take(n).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    };

    let entries = dir.read_dir().map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.path().is_dir();
        if is_dir {
            top_level.push(format!("{name}/"));
            continue;
        }
        top_level.push(name.clone());
        let path = dir.join(&name);
        match name.to_ascii_lowercase().as_str() {
            "package.json" => {
                key_files.push(("package.json".into(), read_snippet(&path, 30)));
                if let Ok(txt) = std::fs::read_to_string(&path) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                        if let Some(dep) = v["dependencies"].as_object() {
                            if dep.contains_key("react") { frameworks.push("React".into()); }
                            if dep.contains_key("vue") { frameworks.push("Vue".into()); }
                            if dep.contains_key("svelte") { frameworks.push("Svelte".into()); }
                            if dep.contains_key("@tauri-apps/api") { frameworks.push("Tauri".into()); }
                            if dep.contains_key("next") { frameworks.push("Next.js".into()); }
                        }
                        if let Some(dev) = v["devDependencies"].as_object() {
                            if dev.contains_key("vite") { frameworks.push("Vite".into()); }
                            if dev.contains_key("typescript") { frameworks.push("TypeScript".into()); }
                        }
                        if let Some(sc) = v["scripts"].as_object().map(|o| o.clone()) {
                            for (k, val) in sc {
                                scripts.push((k, val.as_str().unwrap_or_default().to_string()));
                            }
                        }
                    }
                }
            }
            "cargo.toml" => {
                key_files.push(("Cargo.toml".into(), read_snippet(&path, 30)));
                frameworks.push("Rust".into());
                if std::fs::read_to_string(&path).map(|s| s.contains("tauri")).unwrap_or(false) {
                    frameworks.push("Tauri".into());
                }
            }
            "requirements.txt" => {
                key_files.push(("requirements.txt".into(), read_snippet(&path, 20)));
                frameworks.push("Python (requirements)".into());
            }
            "pyproject.toml" => {
                key_files.push(("pyproject.toml".into(), read_snippet(&path, 20)));
                frameworks.push("Python (pyproject)".into());
            }
            "go.mod" => {
                key_files.push(("go.mod".into(), read_snippet(&path, 10)));
                frameworks.push("Go".into());
            }
            "pom.xml" => {
                key_files.push(("pom.xml".into(), read_snippet(&path, 20)));
                frameworks.push("Java (Maven)".into());
            }
            "build.gradle" => {
                key_files.push(("build.gradle".into(), read_snippet(&path, 20)));
                frameworks.push("Gradle".into());
            }
            "composer.json" => {
                key_files.push(("composer.json".into(), read_snippet(&path, 20)));
                frameworks.push("PHP (Composer)".into());
            }
            "pubspec.yaml" | "pubspec.yml" => {
                key_files.push((name.clone(), read_snippet(&path, 20)));
                frameworks.push("Flutter/Dart".into());
            }
            "vite.config.ts" | "vite.config.js" | "tsconfig.json" | "readme.md" => {
                key_files.push((name.clone(), read_snippet(&path, 15)));
            }
            _ => {}
        }
    }
    let kind = if frameworks.iter().any(|f| f.contains("Python")) {
        "Python project"
    } else if frameworks.iter().any(|f| f == "Rust") {
        "Rust project"
    } else if frameworks.iter().any(|f| f.contains("Java") || f == "Gradle") {
        "Java/JVM project"
    } else if !scripts.is_empty() {
        "Node.js project"
    } else if frameworks.is_empty() {
        "Unknown project"
    } else {
        "Web project"
    };
    frameworks.sort();
    frameworks.dedup();
    Ok(ProjectInspection {
        path: dir.display().to_string(),
        kind: kind.into(),
        frameworks,
        scripts,
        top_level,
        key_files,
    })
}

#[derive(Serialize, Clone)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run an allowlisted build/test command for an inspected project.
#[tauri::command]
pub async fn run_project_command(dir: String, command: String) -> Result<CommandOutput, String> {
    const ALLOWED: &[&str] = &[
        "npm test",
        "npm run test",
        "npm run test:run",
        "npm run lint",
        "npm run typecheck",
        "npm run build",
        "cargo test",
        "cargo build",
        "cargo check",
        "python -m pytest",
        "python -m unittest",
        "go test",
    ];
    if !ALLOWED.contains(&command.as_str()) {
        return Err(format!("command '{command}' is not allowlisted"));
    }
    let dir = dir.trim().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let out = std::process::Command::new("cmd")
            .args(["/C", &command])
            .current_dir(&dir)
            .output()
            .map_err(|e| format!("failed to run command: {e}"))?;
        Ok(CommandOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).chars().take(20000).collect(),
            stderr: String::from_utf8_lossy(&out.stderr).chars().take(20000).collect(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
