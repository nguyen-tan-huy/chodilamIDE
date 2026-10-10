//! jdtls launch config - port of java-debug-model's jdtls_launcher.lua.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use toolchain::{Jdk, Tools};

pub struct JdtlsConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub data_dir: PathBuf,
    pub settings: Value,
    pub init_options: Value,
}

#[derive(Clone, Debug)]
pub struct Memory {
    pub xmx: String,
    pub xms: String,
}

impl Default for Memory {
    /// Phase 0 target: cap the heap (jdtls otherwise may grow to 1/4 of RAM).
    fn default() -> Self {
        Self { xmx: "2G".into(), xms: "256m".into() }
    }
}

/// Own `-data` dir, never nvim's: two jdtls writing one workspace corrupt its index.
pub fn data_dir(root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut h);
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "root".into());
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".cache"));
    cache.join("chodilamide/jdtls").join(format!("{name}-{:08x}", h.finish() as u32))
}

pub fn settings(jdks: &[Jdk]) -> Value {
    let runtimes: Vec<Value> = jdks.iter().map(|j| json!({ "name": j.name, "path": j.path })).collect();
    json!({
        "java": {
            "configuration": { "updateBuildConfiguration": "automatic", "runtimes": runtimes },
            "saveActions": { "organizeImports": false },
            "completion": {
                "favoriteStaticMembers": [
                    "org.junit.jupiter.api.Assertions.*",
                    "org.mockito.Mockito.*",
                    "java.util.Objects.requireNonNull"
                ],
                "importOrder": ["java", "javax", "org", "com", ""]
            },
            "sources": { "organizeImports": { "starThreshold": 5, "staticStarThreshold": 3 } },
            "format": { "enabled": true },
            "signatureHelp": { "enabled": true },
            "import": { "maven": { "enabled": true }, "gradle": { "enabled": true } },
            "autobuild": { "enabled": true }
        }
    })
}

/// Builds the command line and initializationOptions. `module_dirs` are every module the
/// project model knows (reactor AND independent) - passed as initializationOptions
/// `workspaceFolders` at initialize time; adding them later via didChangeWorkspaceFolders
/// was tried in the Lua plugin and broke imports.
pub fn config(root: &Path, module_dirs: &[PathBuf], tools: &Tools, jdks: &[Jdk], mem: &Memory) -> anyhow::Result<JdtlsConfig> {
    let jdtls = tools.jdtls.as_ref().ok_or_else(|| anyhow::anyhow!("không tìm thấy jdtls - chạy: chodilamide-ide-cli tools install"))?;
    let java = toolchain::jdk_for_jdtls(jdks).ok_or_else(|| anyhow::anyhow!("jdtls cần JDK 21+ - không tìm thấy trong /usr/lib/jvm, sdkman, JAVA_HOME"))?;
    let data = data_dir(root);

    // java + the Equinox launcher directly, with the arguments jdtls' bin/jdtls (a Python script)
    // would pass: no Python needed (Windows machines rarely have it)
    let launcher = std::fs::read_dir(jdtls.join("plugins"))
        .ok()
        .and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("org.eclipse.equinox.launcher_") && n.ends_with(".jar")))
        })
        .ok_or_else(|| anyhow::anyhow!("jdtls thiếu plugins/org.eclipse.equinox.launcher_*.jar"))?;
    let os_config = if cfg!(windows) {
        "config_win"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { "config_mac_arm" } else { "config_mac" }
    } else if cfg!(target_arch = "aarch64") {
        "config_linux_arm"
    } else {
        "config_linux"
    };
    let mut args: Vec<String> = Vec::new();
    if java.major >= 24 {
        args.extend(["-Djdk.xml.maxGeneralEntitySizeLimit=0".into(), "-Djdk.xml.totalEntitySizeLimit=0".into()]);
    }
    args.extend([
        "-Declipse.application=org.eclipse.jdt.ls.core.id1".to_string(),
        "-Dosgi.bundles.defaultStartLevel=4".into(),
        "-Declipse.product=org.eclipse.jdt.ls.core.product".into(),
        "-Dosgi.checkConfiguration=true".into(),
        format!("-Dosgi.sharedConfiguration.area={}", jdtls.join(os_config).display()),
        "-Dosgi.sharedConfiguration.area.readOnly=true".into(),
        "-Dosgi.configuration.cascaded=true".into(),
        "-Xms1G".into(),
        "--add-modules=ALL-SYSTEM".into(),
        "--add-opens".into(),
        "java.base/java.util=ALL-UNNAMED".into(),
        "--add-opens".into(),
        "java.base/java.lang=ALL-UNNAMED".into(),
    ]);
    if let Some(l) = &tools.lombok {
        args.push(format!("-javaagent:{}", l.display()));
    }
    if let Some(a) = &tools.patch_agent {
        args.push(format!("-javaagent:{}", a.display()));
    }
    // after -Xms1G above, so these win
    args.push(format!("-Xmx{}", mem.xmx));
    args.push(format!("-Xms{}", mem.xms));
    args.push("-XX:+UseStringDeduplication".into());
    // m2e drops src/test from the classpath when a pom sets <maven.test.skip>true</maven.test.skip>
    // (test files then get no completion/diagnostics). A system property outranks the pom's.
    args.push("-Dmaven.test.skip=false".into());
    args.push("-jar".into());
    args.push(launcher.display().to_string());
    args.push("-data".into());
    args.push(data.display().to_string());

    let mut folders = vec![crate::path_to_uri(root)];
    for m in module_dirs {
        let u = crate::path_to_uri(m);
        if !folders.contains(&u) {
            folders.push(u);
        }
    }
    let settings = settings(jdks);
    let init_options = json!({
        "bundles": tools.bundles,
        "workspaceFolders": folders,
        "settings": settings,
        "extendedClientCapabilities": {
            "classFileContentsSupport": true,
            "progressReportProvider": true,
            "advancedOrganizeImportsSupport": false,
            "generateToStringPromptSupport": true,
            "hashCodeEqualsPromptSupport": true,
            "generateConstructorsPromptSupport": true,
            "advancedGenerateAccessorsSupport": true,
            "overrideMethodsPromptSupport": true,
            "resolveAdditionalTextEditsSupport": true
        }
    });
    Ok(JdtlsConfig {
        program: java.path.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }),
        args,
        env: vec![("JAVA_HOME".into(), java.path.display().to_string())],
        data_dir: data,
        settings,
        init_options,
    })
}

/// Standard `initialize` params with the client capabilities the IDE implements.
pub fn initialize_params(root: &Path, init_options: Value) -> Value {
    let uri = crate::path_to_uri(root);
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "chodilamide", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": uri,
        "workspaceFolders": [{ "uri": uri, "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() }],
        "initializationOptions": init_options,
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "didChangeConfiguration": { "dynamicRegistration": false },
                "applyEdit": true,
                "workspaceEdit": { "documentChanges": true, "resourceOperations": ["create", "rename", "delete"] },
                "symbol": { "dynamicRegistration": false },
                "didChangeWatchedFiles": { "dynamicRegistration": false }
            },
            "textDocument": {
                "synchronization": { "didSave": true, "willSave": false, "dynamicRegistration": false },
                "publishDiagnostics": { "relatedInformation": false, "versionSupport": true },
                "completion": {
                    "completionItem": {
                        "snippetSupport": false,
                        "documentationFormat": ["plaintext", "markdown"],
                        "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] },
                        "insertReplaceSupport": false,
                        "labelDetailsSupport": true
                    },
                    "contextSupport": true
                },
                "hover": { "contentFormat": ["plaintext", "markdown"] },
                "definition": { "linkSupport": false },
                "typeDefinition": { "linkSupport": false },
                "implementation": { "linkSupport": false },
                "references": {},
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "rename": { "prepareSupport": false },
                "formatting": {},
                "codeAction": { "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["quickfix", "refactor", "source", "source.organizeImports"] } } },
                "signatureHelp": { "signatureInformation": { "documentationFormat": ["plaintext"] } },
                "semanticTokens": {
                    "dynamicRegistration": false,
                    "requests": { "full": true, "range": false },
                    "tokenTypes": ["namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator", "decorator", "annotation", "annotationMember", "record", "recordComponent"],
                    "tokenModifiers": ["declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification", "documentation", "defaultLibrary", "public", "private", "protected", "native", "generic", "typeArgument", "importDeclaration", "constructor"],
                    "formats": ["relative"],
                    "overlappingTokenSupport": false,
                    "multilineTokenSupport": false
                },
                "typeHierarchy": { "dynamicRegistration": false },
                "callHierarchy": { "dynamicRegistration": false }
            },
            "window": { "workDoneProgress": false }
        }
    })
}
