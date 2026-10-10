//! Generates Maven + Spring Boot projects and modules (the IDE's "New Project" / "New Module").
//! Pure text generation: no network (it does not call start.spring.io), so it works on a new
//! machine and offline; the Spring Boot version is whatever the user types.
use std::path::{Path, PathBuf};

/// A Spring Boot dependency the wizard offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dep {
    pub id: &'static str,
    pub label: &'static str,
    /// needs a database / external service to start (so the generated test is only a unit test)
    pub needs_service: bool,
}

pub const DEPS: &[Dep] = &[
    Dep { id: "web", label: "Spring Web", needs_service: false },
    Dep { id: "validation", label: "Validation", needs_service: false },
    Dep { id: "actuator", label: "Actuator", needs_service: false },
    Dep { id: "thymeleaf", label: "Thymeleaf", needs_service: false },
    Dep { id: "security", label: "Security", needs_service: false },
    Dep { id: "jpa", label: "Data JPA", needs_service: true },
    Dep { id: "mongodb", label: "Data MongoDB", needs_service: true },
    Dep { id: "mysql", label: "MySQL driver", needs_service: true },
    Dep { id: "postgresql", label: "PostgreSQL driver", needs_service: true },
    Dep { id: "h2", label: "H2 Database", needs_service: false },
    Dep { id: "lombok", label: "Lombok", needs_service: false },
    Dep { id: "devtools", label: "DevTools", needs_service: false },
];

#[derive(Debug, Clone, PartialEq)]
pub struct Coords {
    pub group: String,
    pub artifact: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// one project: Spring Boot parent, one pom
    Single,
    /// a `pom` parent (Spring Boot parent) and its first module
    Multi { module: String },
    /// a module of an existing parent pom (inherits its Spring Boot setup)
    Child(Coords),
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub group: String,
    pub artifact: String,
    pub description: String,
    pub package: String,
    /// 8, 11, 17, 21…
    pub java: u32,
    pub boot: String,
    pub war: bool,
    pub deps: Vec<String>,
    pub kind: Kind,
}

impl Spec {
    fn has(&self, id: &str) -> bool {
        self.deps.iter().any(|d| d == id)
    }

    /// Spring Boot 3+ moved to Jakarta and renamed the MySQL driver.
    fn boot_major(&self) -> u32 {
        self.boot.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(3)
    }

    fn needs_service(&self) -> bool {
        DEPS.iter().any(|d| d.needs_service && self.has(d.id)) && !self.has("h2")
    }
}

/// "demo-api" → "DemoApi"; the class name of the application is that + "Application".
pub fn pascal(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).map(|p| p.chars().next().unwrap().to_uppercase().collect::<String>() + &p[1..]).collect()
}

/// "vn.longvan" + "demo-api" → "vn.longvan.demoapi": a valid, lower-case Java package.
pub fn default_package(group: &str, artifact: &str) -> String {
    let clean = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.').collect::<String>().to_lowercase();
    let mut parts: Vec<String> = clean(group).split('.').filter(|p| !p.is_empty()).map(String::from).collect();
    let a: String = artifact.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    if !a.is_empty() {
        parts.push(a);
    }
    // a package segment must not start with a digit or be a keyword
    parts.into_iter().map(|p| if p.chars().next().is_some_and(|c| c.is_ascii_digit()) || ["default", "class", "package", "public", "new", "int", "long"].contains(&p.as_str()) { format!("_{p}") } else { p }).collect::<Vec<_>>().join(".")
}

/// Errors the wizard shows before writing anything.
pub fn validate(spec: &Spec) -> Result<(), String> {
    let ok_id = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok_id(&spec.group) {
        return Err("Group không hợp lệ (chữ, số, . - _), vd: vn.longvan".into());
    }
    if !ok_id(&spec.artifact) || spec.artifact.starts_with('.') || spec.artifact.contains('/') {
        return Err("Artifact không hợp lệ (chữ, số, - _ .), vd: demo-api".into());
    }
    if spec.package.is_empty() || spec.package.split('.').any(|p| p.is_empty() || p.chars().next().unwrap().is_ascii_digit() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        return Err("Package không hợp lệ, vd: vn.longvan.demoapi".into());
    }
    if spec.boot.trim().is_empty() && !matches!(spec.kind, Kind::Child(_)) {
        return Err("Nhập phiên bản Spring Boot, vd: 2.7.18 hoặc 3.5.0".into());
    }
    if spec.boot_major() >= 3 && spec.java < 17 && !matches!(spec.kind, Kind::Child(_)) {
        return Err("Spring Boot 3 cần Java 17 trở lên: chọn Java 17+ hoặc dùng Spring Boot 2.7.x".into());
    }
    if let Kind::Multi { module } = &spec.kind {
        if !ok_id(module) {
            return Err("Tên module đầu tiên không hợp lệ".into());
        }
    }
    Ok(())
}

fn java_prop(java: u32) -> String {
    if java <= 8 { "1.8".into() } else { java.to_string() }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `<dependency>` blocks for the chosen starters (+ the test starter).
fn dependencies(spec: &Spec) -> String {
    let mut out = String::new();
    let mut add = |group: &str, artifact: &str, extra: &str| {
        out.push_str(&format!("\t\t<dependency>\n\t\t\t<groupId>{group}</groupId>\n\t\t\t<artifactId>{artifact}</artifactId>\n{extra}\t\t</dependency>\n"));
    };
    let boot = "org.springframework.boot";
    for (id, artifact) in [("web", "spring-boot-starter-web"), ("validation", "spring-boot-starter-validation"), ("actuator", "spring-boot-starter-actuator"), ("thymeleaf", "spring-boot-starter-thymeleaf"), ("security", "spring-boot-starter-security"), ("jpa", "spring-boot-starter-data-jpa"), ("mongodb", "spring-boot-starter-data-mongodb")] {
        if spec.has(id) {
            add(boot, artifact, "");
        }
    }
    if spec.has("mysql") {
        if spec.boot_major() >= 3 {
            add("com.mysql", "mysql-connector-j", "\t\t\t<scope>runtime</scope>\n");
        } else {
            add("mysql", "mysql-connector-java", "\t\t\t<scope>runtime</scope>\n");
        }
    }
    if spec.has("postgresql") {
        add("org.postgresql", "postgresql", "\t\t\t<scope>runtime</scope>\n");
    }
    if spec.has("h2") {
        add("com.h2database", "h2", "\t\t\t<scope>runtime</scope>\n");
    }
    if spec.has("lombok") {
        add("org.projectlombok", "lombok", "\t\t\t<optional>true</optional>\n");
    }
    if spec.has("devtools") {
        add(boot, "spring-boot-devtools", "\t\t\t<scope>runtime</scope>\n\t\t\t<optional>true</optional>\n");
    }
    // a war on an external server needs the container provided
    if spec.war && spec.has("web") {
        add(boot, "spring-boot-starter-tomcat", "\t\t\t<scope>provided</scope>\n");
    }
    add(boot, "spring-boot-starter-test", "\t\t\t<scope>test</scope>\n");
    out
}

fn boot_plugin(spec: &Spec) -> String {
    let lombok = if spec.has("lombok") {
        "\t\t\t\t<configuration>\n\t\t\t\t\t<excludes>\n\t\t\t\t\t\t<exclude>\n\t\t\t\t\t\t\t<groupId>org.projectlombok</groupId>\n\t\t\t\t\t\t\t<artifactId>lombok</artifactId>\n\t\t\t\t\t\t</exclude>\n\t\t\t\t\t</excludes>\n\t\t\t\t</configuration>\n"
    } else {
        ""
    };
    format!("\t<build>\n\t\t<plugins>\n\t\t\t<plugin>\n\t\t\t\t<groupId>org.springframework.boot</groupId>\n\t\t\t\t<artifactId>spring-boot-maven-plugin</artifactId>\n{lombok}\t\t\t</plugin>\n\t\t</plugins>\n\t</build>\n")
}

const POM_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<project xmlns=\"http://maven.apache.org/POM/4.0.0\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\n\t\txsi:schemaLocation=\"http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd\">\n\t<modelVersion>4.0.0</modelVersion>\n";

fn boot_parent(boot: &str) -> String {
    format!("\t<parent>\n\t\t<groupId>org.springframework.boot</groupId>\n\t\t<artifactId>spring-boot-starter-parent</artifactId>\n\t\t<version>{}</version>\n\t\t<relativePath/> <!-- lookup parent from repository -->\n\t</parent>\n", xml_escape(boot))
}

fn coords_block(group: &str, artifact: &str, version: &str, spec: &Spec, packaging: Option<&str>) -> String {
    let mut s = format!("\t<groupId>{}</groupId>\n\t<artifactId>{}</artifactId>\n\t<version>{}</version>\n", xml_escape(group), xml_escape(artifact), xml_escape(version));
    if let Some(p) = packaging {
        s.push_str(&format!("\t<packaging>{p}</packaging>\n"));
    }
    s.push_str(&format!("\t<name>{}</name>\n\t<description>{}</description>\n", xml_escape(artifact), xml_escape(if spec.description.is_empty() { artifact } else { &spec.description })));
    s
}

/// pom.xml of a module (or of a single project) that carries the application.
fn app_pom(spec: &Spec, artifact: &str) -> String {
    let mut s = String::from(POM_HEAD);
    let war = spec.war.then_some("war");
    match &spec.kind {
        Kind::Child(p) => {
            s.push_str(&format!("\t<parent>\n\t\t<groupId>{}</groupId>\n\t\t<artifactId>{}</artifactId>\n\t\t<version>{}</version>\n\t</parent>\n", xml_escape(&p.group), xml_escape(&p.artifact), xml_escape(&p.version)));
            // group and version come from the parent: only repeat the group when it differs
            s.push_str(&format!("\t<artifactId>{}</artifactId>\n", xml_escape(artifact)));
            if spec.group != p.group {
                s.push_str(&format!("\t<groupId>{}</groupId>\n", xml_escape(&spec.group)));
            }
            if let Some(w) = war {
                s.push_str(&format!("\t<packaging>{w}</packaging>\n"));
            }
            s.push_str(&format!("\t<name>{}</name>\n\t<description>{}</description>\n", xml_escape(artifact), xml_escape(if spec.description.is_empty() { artifact } else { &spec.description })));
        }
        Kind::Multi { module } => {
            s.push_str(&format!("\t<parent>\n\t\t<groupId>{}</groupId>\n\t\t<artifactId>{}</artifactId>\n\t\t<version>0.0.1-SNAPSHOT</version>\n\t</parent>\n", xml_escape(&spec.group), xml_escape(&spec.artifact)));
            s.push_str(&format!("\t<artifactId>{}</artifactId>\n", xml_escape(module)));
            if let Some(w) = war {
                s.push_str(&format!("\t<packaging>{w}</packaging>\n"));
            }
            s.push_str(&format!("\t<name>{}</name>\n\t<description>{}</description>\n", xml_escape(module), xml_escape(if spec.description.is_empty() { module } else { &spec.description })));
        }
        Kind::Single => {
            s.push_str(&boot_parent(&spec.boot));
            s.push_str(&coords_block(&spec.group, artifact, "0.0.1-SNAPSHOT", spec, war));
            s.push_str(&format!("\t<properties>\n\t\t<java.version>{}</java.version>\n\t</properties>\n", java_prop(spec.java)));
        }
    }
    s.push_str(&format!("\t<dependencies>\n{}\t</dependencies>\n", dependencies(spec)));
    s.push_str(&boot_plugin(spec));
    s.push_str("</project>\n");
    s
}

fn parent_pom(spec: &Spec, module: &str) -> String {
    let mut s = String::from(POM_HEAD);
    s.push_str(&boot_parent(&spec.boot));
    s.push_str(&coords_block(&spec.group, &spec.artifact, "0.0.1-SNAPSHOT", spec, Some("pom")));
    s.push_str(&format!("\t<properties>\n\t\t<java.version>{}</java.version>\n\t</properties>\n", java_prop(spec.java)));
    s.push_str(&format!("\t<modules>\n\t\t<module>{}</module>\n\t</modules>\n</project>\n", xml_escape(module)));
    s
}

fn application_class(spec: &Spec, class: &str) -> String {
    let war = spec.war;
    let mut imports = String::from("import org.springframework.boot.SpringApplication;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n");
    let mut extends = String::new();
    let mut extra = String::new();
    if war {
        imports.push_str("import org.springframework.boot.builder.SpringApplicationBuilder;\nimport org.springframework.boot.web.servlet.support.SpringBootServletInitializer;\n");
        extends = " extends SpringBootServletInitializer".into();
        extra = format!("\n\t@Override\n\tprotected SpringApplicationBuilder configure(SpringApplicationBuilder application) {{\n\t\treturn application.sources({class}.class);\n\t}}\n");
    }
    format!("package {};\n\n{imports}\n@SpringBootApplication\npublic class {class}{extends} {{\n\n\tpublic static void main(String[] args) {{\n\t\tSpringApplication.run({class}.class, args);\n\t}}\n{extra}\n}}\n", spec.package)
}

fn application_test(spec: &Spec, class: &str) -> String {
    format!("package {};\n\nimport org.junit.jupiter.api.Test;\nimport org.springframework.boot.test.context.SpringBootTest;\n\n@SpringBootTest\nclass {class}Tests {{\n\n\t@Test\n\tvoid contextLoads() {{\n\t}}\n\n}}\n", spec.package)
}

fn properties(spec: &Spec, app_name: &str) -> String {
    let mut s = format!("spring.application.name={app_name}\n");
    if spec.has("web") {
        s.push_str("server.port=8080\n");
    }
    if spec.has("mongodb") {
        s.push_str(&format!("spring.data.mongodb.uri=mongodb://localhost:27017/{app_name}\n"));
    }
    if spec.has("jpa") && !spec.has("h2") {
        let (url, driver) = if spec.has("postgresql") {
            (format!("jdbc:postgresql://localhost:5432/{app_name}"), "")
        } else {
            (format!("jdbc:mysql://localhost:3306/{app_name}"), "")
        };
        s.push_str(&format!("spring.datasource.url={url}\nspring.datasource.username=\nspring.datasource.password=\n{driver}spring.jpa.hibernate.ddl-auto=validate\n"));
    }
    s
}

pub const GITIGNORE: &str = "HELP.md\ntarget/\n!.mvn/wrapper/maven-wrapper.jar\n\n### STS / Eclipse ###\n.apt_generated\n.classpath\n.factorypath\n.project\n.settings\n.springBeans\n.sts4-cache\nbin/\n\n### IntelliJ IDEA ###\n.idea\n*.iws\n*.iml\n*.ipr\n\n### NetBeans ###\n/nbproject/private/\n/nbbuild/\n/dist/\n/nbdist/\n/.nb-gradle/\n\n### VS Code ###\n.vscode/\n\n### IDE data ###\n.nvim/\n";

fn module_files(spec: &Spec, artifact: &str, prefix: &str) -> Vec<(String, String)> {
    let class = format!("{}Application", pascal(artifact));
    let pkg_path = spec.package.replace('.', "/");
    let mut v = vec![
        (format!("{prefix}pom.xml"), app_pom(spec, artifact)),
        (format!("{prefix}src/main/java/{pkg_path}/{class}.java"), application_class(spec, &class)),
        (format!("{prefix}src/main/resources/application.properties"), properties(spec, artifact)),
    ];
    // a test that boots the context only when nothing external is needed to start
    if !spec.needs_service() {
        v.push((format!("{prefix}src/test/java/{pkg_path}/{class}Tests.java"), application_test(spec, &class)));
    }
    v
}

/// Every file of the project, relative to its folder.
pub fn files(spec: &Spec) -> Vec<(String, String)> {
    match &spec.kind {
        Kind::Single => {
            let mut v = module_files(spec, &spec.artifact, "");
            v.push((".gitignore".into(), GITIGNORE.into()));
            v
        }
        Kind::Multi { module } => {
            let mut v = vec![("pom.xml".to_string(), parent_pom(spec, module)), (".gitignore".to_string(), GITIGNORE.to_string())];
            v.extend(module_files(spec, module, &format!("{module}/")));
            v
        }
        // the module's own folder: the caller places it under the parent
        Kind::Child(_) => module_files(spec, &spec.artifact, ""),
    }
}

/// Writes the project into `dir` (created); refuses a folder that already has files.
pub fn write(dir: &Path, spec: &Spec) -> Result<Vec<PathBuf>, String> {
    validate(spec)?;
    if dir.exists() && std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{} đã tồn tại và không trống", dir.display()));
    }
    let mut written = Vec::new();
    for (rel, text) in files(spec) {
        let path = dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// The text between two tags, first occurrence.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>"))? + start;
    Some(text[start..end].trim())
}

/// groupId / artifactId / version of a pom (own, else its parent's) and its packaging.
pub fn read_pom(text: &str) -> Option<(Coords, String)> {
    let (parent, rest) = match (text.find("<parent>"), text.find("</parent>")) {
        (Some(a), Some(b)) if a < b => (Some(&text[a..b]), format!("{}{}", &text[..a], &text[b + "</parent>".len()..])),
        _ => (None, text.to_string()),
    };
    // own coordinates come before dependencies / dependencyManagement / build
    let head_end = ["<dependencies>", "<dependencyManagement>", "<build>", "<profiles>"].iter().filter_map(|t| rest.find(t)).min().unwrap_or(rest.len());
    let head = &rest[..head_end];
    let artifact = tag(head, "artifactId")?.to_string();
    let group = tag(head, "groupId").or_else(|| parent.and_then(|p| tag(p, "groupId")))?.to_string();
    let version = tag(head, "version").or_else(|| parent.and_then(|p| tag(p, "version")))?.to_string();
    let packaging = tag(head, "packaging").unwrap_or("jar").to_string();
    Some((Coords { group, artifact, version }, packaging))
}

/// Does this pom (or its parent) bring Spring Boot's dependency / plugin management?
pub fn is_spring_boot_pom(text: &str) -> bool {
    text.contains("spring-boot-starter-parent") || text.contains("spring-boot-dependencies")
}

/// `<module>` added to a parent pom (`<modules>` created when missing).
pub fn add_module_to_pom(text: &str, module: &str) -> Result<String, String> {
    if text.contains(&format!("<module>{module}</module>")) {
        return Ok(text.to_string());
    }
    let entry = format!("\t\t<module>{}</module>\n", xml_escape(module));
    if let Some(i) = text.find("</modules>") {
        // keep the closing tag's own indentation
        let line_start = text[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let at = if text[line_start..i].trim().is_empty() { line_start } else { i };
        let mut out = text.to_string();
        out.insert_str(at, &if at == i { format!("\n{entry}") } else { entry });
        return Ok(out);
    }
    let Some(end) = text.rfind("</project>") else { return Err("pom.xml không có </project>".into()) };
    let mut out = text.to_string();
    out.insert_str(end, &format!("\t<modules>\n{entry}\t</modules>\n"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: Kind) -> Spec {
        Spec { group: "vn.demo".into(), artifact: "demo-api".into(), description: "Demo".into(), package: default_package("vn.demo", "demo-api"), java: 17, boot: "3.5.0".into(), war: false, deps: vec!["web".into(), "lombok".into(), "jpa".into(), "mysql".into()], kind }
    }

    #[test]
    fn names_are_derived() {
        assert_eq!(pascal("demo-api"), "DemoApi");
        assert_eq!(pascal("computing_service"), "ComputingService");
        assert_eq!(default_package("vn.longvan", "demo-api"), "vn.longvan.demoapi");
        assert_eq!(default_package("Vn.LongVan", "3d-app"), "vn.longvan._3dapp");
    }

    #[test]
    fn validation_catches_mistakes() {
        let mut s = spec(Kind::Single);
        assert!(validate(&s).is_ok());
        s.java = 11;
        assert!(validate(&s).unwrap_err().contains("Java 17"), "boot 3 on java 11");
        s.boot = "2.7.18".into();
        assert!(validate(&s).is_ok());
        s.artifact = "bad name".into();
        assert!(validate(&s).is_err());
        s.artifact = "ok".into();
        s.package = "1bad.pkg".into();
        assert!(validate(&s).is_err());
    }

    #[test]
    fn single_project_files() {
        let s = spec(Kind::Single);
        let f: std::collections::BTreeMap<_, _> = files(&s).into_iter().collect();
        let pom = &f["pom.xml"];
        assert!(pom.contains("<version>3.5.0</version>") && pom.contains("<java.version>17</java.version>"));
        assert!(pom.contains("spring-boot-starter-web") && pom.contains("mysql-connector-j") && pom.contains("<optional>true</optional>"));
        assert!(pom.contains("spring-boot-starter-test"));
        assert!(f.contains_key("src/main/java/vn/demo/demoapi/DemoApiApplication.java"));
        assert!(!f.keys().any(|k| k.contains("Tests.java")), "JPA + MySQL need a database: no context test");
        let mut web = s.clone();
        web.deps = vec!["web".into()];
        assert!(files(&web).iter().any(|(k, _)| k.ends_with("DemoApiApplicationTests.java")));
        // boot 2 names the MySQL driver differently
        let mut old = s;
        old.boot = "2.7.18".into();
        old.java = 11;
        assert!(files(&old).iter().find(|(k, _)| k == "pom.xml").unwrap().1.contains("mysql-connector-java"));
    }

    #[test]
    fn multi_project_and_child_module() {
        let m = spec(Kind::Multi { module: "demo-api-app".into() });
        let f: std::collections::BTreeMap<_, _> = files(&m).into_iter().collect();
        assert!(f["pom.xml"].contains("<packaging>pom</packaging>") && f["pom.xml"].contains("<module>demo-api-app</module>"));
        let child = &f["demo-api-app/pom.xml"];
        assert!(child.contains("<artifactId>demo-api</artifactId>") && child.contains("<relativePath") == false);
        assert!(!child.contains("spring-boot-starter-parent"), "the module inherits from the parent pom");
        // a child of an existing parent: group repeated only when it differs
        let p = Coords { group: "vn.demo".into(), artifact: "demo-api".into(), version: "1.2.0".into() };
        let c = files(&spec(Kind::Child(p.clone())));
        let pom = &c.iter().find(|(k, _)| k == "pom.xml").unwrap().1;
        assert!(pom.contains("<version>1.2.0</version>") && !pom.contains("<java.version>"));
        let mut other = spec(Kind::Child(p));
        other.group = "vn.other".into();
        assert!(files(&other).iter().find(|(k, _)| k == "pom.xml").unwrap().1.contains("<groupId>vn.other</groupId>"));
    }

    #[test]
    fn reads_poms_and_adds_modules() {
        let parent = "<project>\n\t<parent>\n\t\t<groupId>org.springframework.boot</groupId>\n\t\t<artifactId>spring-boot-starter-parent</artifactId>\n\t\t<version>2.7.12</version>\n\t</parent>\n\t<groupId>vn.x</groupId>\n\t<artifactId>root</artifactId>\n\t<version>1.0</version>\n\t<packaging>pom</packaging>\n\t<modules>\n\t\t<module>a</module>\n\t</modules>\n\t<dependencies><dependency><groupId>g</groupId><artifactId>dep</artifactId><version>9</version></dependency></dependencies>\n</project>\n";
        let (c, pk) = read_pom(parent).unwrap();
        assert_eq!((c.group.as_str(), c.artifact.as_str(), c.version.as_str(), pk.as_str()), ("vn.x", "root", "1.0", "pom"));
        assert!(is_spring_boot_pom(parent));
        let with = add_module_to_pom(parent, "b").unwrap();
        assert!(with.contains("<module>a</module>") && with.contains("<module>b</module>"));
        assert_eq!(add_module_to_pom(&with, "b").unwrap(), with, "no duplicates");
        let bare = "<project>\n\t<groupId>g</groupId>\n\t<artifactId>r</artifactId>\n\t<version>1</version>\n\t<packaging>pom</packaging>\n</project>\n";
        assert!(add_module_to_pom(bare, "m").unwrap().contains("<modules>\n\t\t<module>m</module>\n\t</modules>"));
        // coordinates from the parent when the child omits them
        let child = "<project><parent><groupId>g</groupId><artifactId>p</artifactId><version>3</version></parent><artifactId>c</artifactId></project>";
        assert_eq!(read_pom(child).unwrap().0, Coords { group: "g".into(), artifact: "c".into(), version: "3".into() });
    }

    #[test]
    fn write_refuses_a_busy_folder() {
        let dir = std::env::temp_dir().join(format!("scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = spec(Kind::Single);
        let written = write(&dir, &s).unwrap();
        assert!(written.iter().any(|p| p.ends_with("pom.xml")));
        assert!(write(&dir, &s).is_err(), "second write into a non-empty folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
