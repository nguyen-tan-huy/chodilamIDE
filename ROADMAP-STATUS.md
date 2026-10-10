# ChoDiLamIDE roadmap - trạng thái
Cập nhật: 2026-09-29

Kế hoạch chi tiết: skill `chodilamide-ide` (~/.claude/skills/chodilamide-ide).

## Quyết định
- Viết lại IDE bằng Rust, không tự viết phần hiểu code Java: dùng jdtls qua LSP (thử `nbcode` sau).
- Debugger: tự viết bằng Java dùng JDI, nói DAP, không phụ thuộc jdtls.
- Repo IDE Rust: ~/Git-projects/chodilamide (mặc định, chưa git init)
- Kiểu phím: IntelliJ (mặc định do chưa trả lời, 29/9 - đổi được)
- Framework UI: GPUI (theo Zed, chốt 29/9)
- Ngôn ngữ ban đầu: Java + pom + .xhtml (chốt 29/9)

## Giai đoạn
- [ ] 0 Tối ưu jdtls - số đo trước (28/9): product-service RSS 2.29 GB (heap 0.87/1.63 GB),
      computing-management 1.59 GB; -Xms1G, không -Xmx (MaxHeap 7.3 GB). Sau: -
- [x] 1 project-model (29/9) - crates project-model + cli `chodilamide model inspect|classpath`.
      Kết quả --force giống hệt model-cache.json của Lua (product-service 6 module, axigen 2).
      Cache hit: 99 ms / 11 ms, không gọi mvn; đổi mtime pom -> resolve lại cả project (như Lua).
      Giữ nguyên lỗi của Lua: deps lấy khối <dependencies> ĐẦU TIÊN (thường là
      dependencyManagement, ~1200 deps/module) - sửa đồng thời ở cả hai bên.
- [x] 2 editor (GPUI 0.2.2) - người dùng xác nhận chạy tốt 29/9; còn lẻ tẻ ở dưới, app chạy được: `target/release/chodilamide-ide <dir>`
      [x] bước 1 IME: người dùng gõ thử tiếng Việt qua fcitx5 trong ime_probe -> OK (29/9)
      [x] crate editor-core (ropey, undo nhóm, grapheme, IME, keymap IntelliJ) - 7/7 test
      [x] crate syntax: tree-sitter Java + XML (pom.xml, .xhtml), query đi kèm grammar - 2/2 test
          (chưa dùng highlights.scm của nvim-treesitter; parse lại cả file mỗi lần sửa)
      [x] app: cây project từ project-model (module/source root/package gộp/resources/webapp/pom,
          nhóm Open Editors), tab, vùng trống khi chưa mở file, status bar, settings.toml
          (font_size theo point như wezterm, mặc định 12), đọc model nền không chặn UI,
          lưu pom.xml -> resolve lại model. Test tree + tab: 2/2.
      [x] đo: mở product-service, cây 6 module hiện ngay từ cache; RSS app 108 MB.
      [x] người dùng kiểm tay UI: mở/sửa/lưu, màu, cuộn -> "đúng ngon" (29/9)
      [x] thêm: ô nhập 1 dòng có IME (input.rs), thanh tìm Ctrl+F (tô mọi kết quả, Enter/F3,
          Esc), Ctrl+Shift+N mở file fuzzy (nucleo, bỏ dấu), Ctrl+Shift+F tìm trong project
          (ignore + tìm chuỗi, nền, tối đa 500), hỏi Lưu/Không lưu/Hủy khi đóng tab chưa lưu.
          Test toàn workspace 16/16. CHỜ người dùng kiểm tay các phần mới.
      [ ] còn: hỏi lưu khi THOÁT app (Ctrl+Q/đóng cửa sổ vẫn bỏ thay đổi), tìm ngược (Shift+F3),
          thay thế (Ctrl+R), icon Nerd Font, nhiều con trỏ, parse tree-sitter tăng dần.
- [ ] 3 lsp-client - đang làm (29/9):
      [x] crate toolchain (tìm jdtls 1.54.0 vá, java-debug/java-test, lombok, agent vá, JDK 11/21/26)
      [x] crate lsp-client: JSON-RPC stdio tự viết (thread + futures, KHÔNG dùng async-lsp/tokio
          để chạy được trên executor của GPUI), $/cancelRequest, didChange tăng dần (test replay),
          jdtls: -data ~/.cache/chodilamide/jdtls/<tên>-<hash> (tách nvim), -Xmx2G -Xms256m.
      [x] `chodilamide lsp check`: product-service ServiceReady 12s, BaseBean 0 lỗi (lombok OK),
          definition product-web -> product-core/ConfigProperty.java (source) 375ms, RSS 1.36-1.43GB.
      [x] app: tự chạy jdtls sau khi có model; gạch lượn lỗi/cảnh báo, F2/Shift+F2, thông báo ở
          status bar; Ctrl+B/F12/Ctrl+Click định nghĩa (kể cả class trong jar jdt:// chỉ đọc);
          Ctrl+Alt+←/→ lịch sử; Ctrl+Q hover; Ctrl+Space + tự gợi ý sau '.'/2 chữ, auto-import
          qua resolve; Alt+F7 chỗ dùng; Alt+6 Problems; Ctrl+Alt+L format; Ctrl+Alt+O import.
          Thoát app: shutdown jdtls. Thoát = Ctrl+Shift+Q (Ctrl+Q giờ là hover như IntelliJ).
          Ảnh chụp: status bar "jdtls ✓" sau import. 19/19 test.
      [x] thêm (29/9): Alt+Enter code action/quick fix (resolve + executeCommand + applyEdit),
          Shift+F6 đổi tên (WorkspaceEdit đầy đủ: tab mở sửa trong buffer, file chưa mở sửa trên
          đĩa, đổi tên file .java - cần resourceOperations, đã khai báo), Ctrl+N symbol
          (workspace/symbol, file project trước), Ctrl+F12 cấu trúc file, hỏi lưu khi thoát
          (Ctrl+Shift+Q hoặc đóng cửa sổ). CLI kiểm: rename ConfigProperty -> 10 file + 1 đổi tên
          (chỉ hỏi, không áp dụng); symbol 246ms; mở lại project: ServiceReady 2.8s, RSS ~0.5-1GB.
          Test 20/20.
      [ ] CHỜ người dùng kiểm tay (chưa ai bấm thử trong UI).
      [ ] còn: signature help, tài liệu trong popup gợi ý, Ctrl+N chưa mở được class trong jar,
          panel Problems cố định, thử nbcode.
- [ ] 4 run/debug/test/maven - đang làm (29/9):
      [x] 4a debugger JDI riêng `debugger/` (Java 17, không thư viện ngoài, build.sh ->
          build/chodilamide-debugger.jar). DAP qua stdio, cùng tên trường launch với bản nvim
          (mainClass/classPaths/sourcePaths/vmArgs/args/env/cwd/javaExec/noDebug) + request
          redefineClasses. Có: breakpoint đặt trước khi class nạp, điều kiện, logpoint, step
          over/in/out, stack, biến (collection/map hiện phần tử), evaluate (field, method, so sánh),
          exception uncaught, hot swap, output, exit code. test/dap_test.py: 17/17 (app JDK 11,
          debugger JDK 21, breakpoint ở module khác). Không cần jdtls.
      [x] 4b crate runner: đọc/ghi CHUNG debug-configs.json + active-config.json + maven-jdk.json
          (giữ trường lạ, env [] của Lua), classpath/sourcepath port jdtls.lua (module anh em dùng
          target/classes). `chodilamide run list|show` đọc đúng 4 config thật của product-service.
          crate dap-client + test tích hợp chạy debugger thật. Tổng 24 test Rust.
      [x] 4c UI (build OK, CHƯA ai bấm thử): chấm đỏ breakpoint ở gutter / Ctrl+F8 (lưu
          .nvim/java-debug-model/chodilamide-breakpoints.json), ▶ cạnh main -> Run/Debug (tự tạo
          config vào file chung), chọn config + ▶ 🐞 ■ trên thanh tab, Shift+F10/Shift+F9/Ctrl+F2,
          F7/F8/Shift+F8/F9, Ctrl+F9 build + hot swap, Alt+4 panel: console (link File.java:N),
          frames, cây biến; tab Maven (lifecycle theo module, bỏ qua test). Build trước khi chạy:
          jdtls java/buildWorkspace, không có jdtls thì mvn -o compile. Nhiều phiên cùng lúc.
      [x] test runner (29/9): qua Maven Surefire (JUnit 4+5, đúng cấu hình build, không cần
          jdtls): runner::test (mvn test -Dtest=Cls#m, đọc TEST-*.xml, chạy lại test lỗi), ▶ ở gutter
          cạnh @Test / class test -> Run/Debug, phiên Test có cây ✓/✖ + lỗi, bấm để tới dòng lỗi,
          nút "Chạy lại test lỗi". Debug test: -Dmaven.surefire.debug + debugger attach (thử lại
          tới khi JVM mở cổng, vì Surefire 3 không in dòng Listening). Kiểm trên project mẫu JUnit 5
          (scratchpad, offline): CLI `chodilamide test` 1.4s/vòng, rerun chỉ chạy test lỗi;
          debug test dừng đúng dòng, đọc biến. KHÔNG chạy test thật của product-service (có thể gọi
          DB/dịch vụ). Test Rust 25/25, DAP 17/17.
      [x] chạy test KHÔNG qua Maven (người dùng yêu cầu 29/9: "maven chậm"): debugger/testrunner
          (build/chodilamide-testrunner.jar, --release 8) chạy JUnit Platform + JUnitCore cho class
          JUnit 4 (product-service có junit-4 nhưng không có vintage engine), in "@@CDL {json}".
          IDE launch nó qua debugger (Run = noDebug, Debug = breakpoint như app), tự thêm
          junit-platform-launcher đúng version engine từ ~/.m2 (product-service: 1.8.2).
          testrunner_test.py 7/7: 2 class JUnit5+JUnit4 0.58s (Maven ~1.4s trên project mẫu),
          debug 1 method dừng đúng dòng. Build trước test: jdtls hoặc mvn -o test-compile.
      [ ] CHỜ người dùng kiểm tay toàn bộ giai đoạn 4 trong UI.
      [ ] còn: watches, sửa config trong UI, danh sách thread, attach vào cổng có sẵn.
- [ ] 5 vỏ + đóng gói - đang làm (29/9):
      [x] vỏ: mỗi project một tab trong 1 cửa sổ, "+"/Ctrl+Shift+T = bảng chọn project (● đang mở,
          ◷ gần đây, git repo trong MYAPP_PROJECT_ROOTS sâu MYAPP_PROJECT_DEPTH, MYAPP_PROJECTS,
          myapp.env; gõ đường dẫn để mở thư mục khác), Ctrl+Shift+W đóng project (hỏi lưu),
          Ctrl+Shift+]/[ chuyển project, Ctrl+Shift+Q thoát (hỏi lưu mọi project). Đọc/ghi CHUNG
          ~/.local/state/chodilamide/projects.json + recent.json với wezterm.lua; không tham số
          -> mở lại các project đang mở. jdtls chỉ chạy khi project được hiện lần đầu.
      [x] session: file mở + con trỏ theo project, CÙNG file JSON với chodilamide_session.lua
          (~/.local/state/chodilamide-nvim/chodilamide-sessions/), lưu 30s/đóng tab/thoát.
      [x] gói Arch packaging/arch/PKGBUILD -> chodilamide-ide-0.1.0-1-x86_64.pkg.tar.zst (9 MB):
          /usr/bin/chodilamide-ide, chodilamide-ide-cli, /usr/share/chodilamide/*.jar, .desktop,
          icon. Cài song song gói chodilamide cũ. Kiểm: giải nén thử, CLI tìm đúng jar đi kèm.
          CHƯA cài (cần sudo: người dùng tự chạy pacman -U).
      [x] `chodilamide-ide-cli doctor` (jdtls, lombok, agent vá, JDK 21+/17+, debugger, testrunner,
          mvn, git + cách sửa) và `tools install [--jdk]` (curl+tar vào ~/.local/share/chodilamide/tools:
          jdtls 1.54.0 bản vá cùng URL với java-debug-model, lombok, JDK 21 Temurin). Giả lập máy mới
          (XDG_DATA_HOME tạm): doctor ✗ -> install (jdtls 51MB, lombok 2MB) -> doctor ✓ -> jdtls tải về
          chạy product-service: ServiceReady 10s, 0 lỗi, định nghĩa xuyên module OK. URL JDK: HTTP 200
          (chưa tải thử 207MB). Agent vá jdtls đóng kèm gói (packaging/common).
      [x] terminal tích hợp (29/9): alacritty_terminal 0.26 (PTY + VT), vẽ bằng GPUI, 256 màu/truecolor,
          cuộn lịch sử 10k dòng, bôi chọn chuột, Ctrl+Shift+C/V (dán bracketed), gõ tiếng Việt qua IME
          (commit; preedit chưa vẽ trong lưới). Alt+F12 bật/tắt, tab Terminal trong panel dưới, nhiều
          terminal/project, mở ở thư mục project. Phím vào shell trước phím tắt IDE (interceptor), trừ
          Ctrl+Shift+*, Alt+số, Alt+F12. Test: PTY /bin/sh in "xin chào 42", bảng phím VT. 31 test Rust.
          CHƯA ai dùng thử trong UI (lazygit, vim trong terminal...).
      [x] gói Arch build lại (9.6 MB) có agent vá. CHƯA cài (người dùng tự pacman -U).
      [ ] còn: gộp CLI vào 1 binary, `update`, AppImage, Windows .exe, macOS .app (không test được tại
          chỗ), preedit IME trong terminal, git init repo.

- [x] Yêu cầu 30/9 (build OK, CHƯA ai bấm thử trong UI):
      [x] breakpoint cho cả debug app và debug unit test: SỬA LỖI đặt/bỏ breakpoint khi đang debug test
          không gửi tới phiên test (chỉ gửi phiên Kind::Debug) - giờ gửi mọi phiên `debugging`; hot swap
          cũng áp dụng cho phiên debug test. Breakpoint có điều kiện: chuột phải chấm đỏ / Ctrl+Shift+F8,
          chấm màu cam, lưu (chodilamide-breakpoints.json v2, đọc được v1). testrunner_test.py 9/9 (thêm:
          điều kiện đúng -> dừng, sai -> chạy hết).
      [x] UI sửa cấu hình chạy (Alt+Shift+F10 / "✎ Sửa cấu hình…" trong ô chọn config): danh sách + form
          (tên, main class, thư mục chạy, VM options, program args, env "K=V,K2=V2" như form nvim, Maven
          profiles, module, JDK), Mới/Nhân bản/Xóa/Lưu/Lưu & Run/Debug, kiểm tra trùng tên/thiếu main class.
          Ghi debug-configs.json dùng chung, giữ trường lạ.
      [x] UI cài đặt (Ctrl+Alt+S / "⚙ Cài đặt" trên thanh project): theme Darcula / Light / Tokyo Night
          (xem trước ngay, Hủy thì trả lại), font editor/giao diện (danh sách font monospace đã cài hoặc gõ tên),
          cỡ chữ, khoảng cách dòng, tab, độ rộng cây, chiều cao panel, jdtls -Xmx/-Xms, bật/tắt jdtls.
          Áp dụng ngay cho mọi project, lưu settings.toml. 35 test Rust.

- [x] Sửa UI theo phản hồi 30/9 ("UI quá tệ, font hiển thị tất cả k chỉ là mono"): khung hộp thoại mới
      (thân cuộn, nút cố định, không tràn), nhóm Giao diện/Editor/Java, nhãn không xuống dòng, ô số hẹp,
      ô nhập chỉ tô chọn/viền xanh khi đang focus, font chọn bằng dropdown có lọc liệt kê TẤT CẢ font
      (mỗi tên vẽ bằng chính font đó). Sửa lỗi hàng Module trống khi mở dialog trước lúc có model.
      Tự kiểm bằng ảnh chụp (instance riêng: XDG_CONFIG_HOME/XDG_STATE_HOME tạm, jdtls tắt,
      CHODILAMIDE_OPEN=settings|settings-font|configs mở sẵn dialog để chụp).

- [x] Maven như java-project-model (30/9): cửa sổ Maven mới (tab Maven, Alt+4): cây reactor + "Lifecycle
      (toàn bộ reactor)" + module (độc lập để riêng, [independent]), đủ phase clean…site…deploy; chọn JDK
      cho Maven (ghi maven-jdk.json dùng chung, đặt JAVA_HOME + PATH như maven_jdk.lua); Maven profiles
      (dùng khi chạy và khi reload model, lưu chodilamide-maven.json cùng Skip Tests); goal tùy ý cho module
      đang chọn (:JavaMavenGoal); dependency tree (-Dverbose, lọc giữ nhánh cha, xung đột đỏ, trùng mờ);
      Reload model; thêm/bỏ module qua manifest.json (:JavaModelAddModule/RemoveModule). Kiểm bằng ảnh
      trên project mẫu: cây + dependency tree thật 19 dòng. Test runner::maven 2 test mới.
      [x] sửa: chữ trong ô nhập tràn đè nút bên cạnh (cắt theo khung) - CHƯA chụp lại kiểm.
      [x] sửa lỗi người dùng gặp: ▶ Run class có main() trong src/test/java (ComputingCoreApplicationTest
          ở computing-management) báo ClassNotFound vì thiếu target/test-classes -> giờ dùng classpath test
          + build test-compile. 38 test Rust.

- [x] UI theo ảnh 30/9 00:26 (build OK, 39 test, CHƯA chụp kiểm vì người dùng đang dùng app):
      kéo thả đổi kích thước cây project / panel dưới / Maven|dependency tree (lưu
      .nvim/java-debug-model/chodilamide-layout.json, kẹp min/max); indent guide như IntelliJ (dòng trống theo
      khối xung quanh, màu theo theme); gutter tách cột số dòng | breakpoint | ▶ (hết đè số, bấm cột ▶ mới mở
      menu Run/Debug, bấm chỗ khác trong gutter đặt breakpoint); cây project rộng mặc định 340, "[independent]"
      chữ mờ nhỏ; log Run/Debug: không còn ép cuộn xuống cuối mỗi dòng mới (lỗi không cuộn được khi đang
      chạy) - "Theo dõi cuối" bật/tắt, cuộn lên tự tắt, "Xuống dòng" (3000 dòng cuối), "Xóa log".

- [x] Menu chuột phải + dependencies + file ẩn (30/9; build OK, 39 test, CHƯA chụp kiểm - người dùng đang dùng app):
      menu kiểu IntelliJ ở editor (Go to Definition, Find Usages, Context Actions, Rename, Reformat, Optimize
      Imports, Cut/Copy/Paste, breakpoint, Run/Debug file, Copy Path; chuột phải ngoài vùng chọn thì dời con trỏ),
      cây (New Java Class có package tự điền / New File / New Directory-package "a.b.c" / Rename (tab đang mở đi
      theo) / Delete (hỏi, đóng tab file bị xóa) / Copy Path / mở trình quản lý file / mở terminal ở đây / Maven),
      tab (Close / Close Others / Close All / Copy Path), console (Copy log / Xóa / Xuống dòng), module Maven.
      Cây: "Dependencies (N)" mỗi module liệt kê jar; chuột phải jar -> xóa thư mục version khỏi ~/.m2 (hỏi xác
      nhận, CHỈ trong ~/.m2/repository). Module hiện mọi file cấp module (pom, README, .gitignore…), nút
      "Thư mục gốc", nút "○/◉ file ẩn" trên header cây (dotfiles, target, .git).

- [x] Sửa "xóa Dependencies không thành công" (30/9): kiểm đĩa -> không jar nào bị xóa, lệnh xóa chưa từng chạy.
      Trên Wayland GPUI không có prompt hệ thống (WaylandWindow::prompt trả None, dùng hộp tự vẽ dự phòng) -
      bỏ window.prompt cho Delete file / xóa jar, thay bằng hộp xác nhận riêng của app (Enter xóa, Esc hủy).
      Tách delete_jar/jar_version_dir có test (chỉ trong ~/.m2/repository, >= group/artifact/version, không "..").
      Jar đã xóa hiện "(đã xóa)" tới lần Reload. CHƯA kiểm tay trong UI. Còn 2 chỗ dùng window.prompt
      (đóng tab chưa lưu, đóng project/thoát) - cần hỏi người dùng hộp đó có hiện không.

- [x] Reload Maven tải lại jar đã xóa + nạp lại project (30/9): nút Reload / Ctrl+Shift+O / menu chạy lại mvn
      online (dependency:build-classpath tải jar thiếu), làm mới cây (hết "(đã xóa)"), gửi jdtls
      java/projectConfigurationsUpdate cho pom mọi module, báo "jar đầy đủ" hoặc số jar còn thiếu. Kiểm thật với
      kho Maven tạm (MAVEN_OPTS=-Dmaven.repo.local, không đụng ~/.m2): xóa thư mục junit-jupiter-params 5.10.2 ->
      reload -> jar quay lại. Phần jdtls cập nhật classpath CHƯA kiểm với jdtls thật. 41 test Rust.

- [x] Mọi thay đổi trên đĩa đều báo jdtls (30/9): workspace/didChangeWatchedFiles cho New Java Class/File/Directory
      (created), Delete (deleted; xóa pom/module -> reload + restart jdtls), Rename (deleted+created), lưu file
      không phải Java (changed), refactor ghi xuống file chưa mở (changed/created/deleted); pom trong các thay
      đổi -> projectConfigurationsUpdate. Xóa jar -> projectConfigurationsUpdate cho module dùng jar đó. Lưu
      pom.xml -> reload đầy đủ (có báo jdtls). Maven chạy xong -> update mọi module + java/buildWorkspace.
      Thêm/bỏ module -> restart jdtls sau reload (module list chỉ nạp lúc khởi động). Khai báo capability
      didChangeWatchedFiles. Kiểm với jdtls thật (`chodilamide lsp watch-check`, project mẫu): tạo file + báo ->
      Ctrl+N thấy class; xóa + báo -> mất; update + buildWorkspace -> OK. 41 test Rust.

- [x] 30/9 ~01:30: theo dõi thư mục project + sửa thật lỗi xóa jar.
      NGUYÊN NHÂN xóa jar: hộp xác nhận mở từ cú bấm menu; gpui đặt lại focus cho vùng được bấm khi nhả chuột
      -> Enter ("Xóa (Enter)") không tới hộp thoại, Esc/bấm ngoài thì hủy -> không xóa. Sửa: dialogs::focus_later
      (window.defer) cho MỌI hộp thoại (xác nhận, New/Rename, breakpoint, cấu hình chạy, cài đặt, picker, chọn
      project) + focus nằm trong key context "Dialog". Chứng minh bằng test UI (gpui test-support, mô phỏng chuột
      + phím trên cửa sổ ảo): code cũ FAIL, code mới PASS. Thêm debug_selector cho dòng cây/mục menu/nút.
      Theo dõi thư mục (notify 8, inotify): bỏ target/.git/node_modules/.idea/.nvim + file tạm, gom 300ms; báo jdtls
      didChangeWatchedFiles; làm mới cây; tab mở tự nạp lại nếu chưa sửa (đang sửa -> chỉ cảnh báo); pom đổi ->
      reload. Test UI: sửa file ngoài -> tab nạp lại, không dirty. Test inotify thật. Test UI chạy tuần tự
      (--test-threads=1: env CHODILAMIDE_M2_REPO dùng chung). 47 test Rust.

- [x] 30/9 ~01:25: (1) xóa jar "không mất": thực ra ĐÃ xóa nhưng jdtls/m2e tải lại ngay vì app báo
      projectConfigurationsUpdate (2captcha-java/1.1.1 tạo lại 01:15:32, 1 phút sau khi mở app). Người dùng chọn
      "xóa hẳn, tải lại khi Reload": lúc xóa KHÔNG báo jdtls; Reload chạy Maven online rồi báo jdtls.
      (2) "mvnw: Permission denied" (storefront-service/mvnw -rw-r--r--): runner::maven::launcher dùng mvnw nếu
      có quyền chạy, không thì `sh mvnw …` (không đổi quyền file), không có mvnw -> mvn; Windows mvnw.cmd.
      Kiểm: `sh mvnw --version` ở storefront-service -> Maven 3.8.7 của wrapper. 48 test Rust.

- [x] Xem giá trị khi debug kiểu IntelliJ (30/9; build OK, 50 test, CHƯA kiểm tay/chụp): inline values (chữ nghiêng
      xám "name: value" cuối dòng nhắc tới biến lần cuối trước dòng dừng, 80 dòng, bỏ comment/this); rê chuột lên
      biến/chuỗi a.b.c 350ms -> popup giá trị, mở ▸ xem field/phần tử (dùng cache variables của phiên); Alt+F8
      Evaluate Expression (lấy chữ đang chọn hoặc biểu thức tại con trỏ, lịch sử 20 kết quả mở rộng được); Watches
      trong panel biến (thêm bằng Enter, ×, tính lại mỗi lần dừng/đổi frame, lưu chodilamide-watches.json).
      Tất cả qua DAP evaluate trong frame đang chọn; chạy tiếp/step thì xóa giá trị cũ. Chạy cho cả debug unit test.

- [x] 30/9 ~02:00 "Debug trên thanh active profile mà profile là unit test chạy như debug thường" + ClassNotFound
      ComputingCoreApplicationTest (computing-management): GỐC RỄ computing-core/pom.xml có
      <maven.test.skip>true</maven.test.skip> -> Maven không biên dịch test, jdtls .classpath không có src/test/java
      -> target/test-classes trống. Sửa: ensure_built kiểm file .class sau build; thiếu -> javac chỉ đúng file cần
      chạy (-sourcepath, như IntelliJ; bỏ qua file test hỏng như NetworkProfileControllerTest) bằng JDK của config
      / maven-jdk.json -> còn thiếu -> mvn [test-]compile -Dmaven.test.skip=false (offline rồi online). Kiểm thật:
      `chodilamide compile` ComputingCoreApplicationTest JDK 11: 1.3s, .class có. (mvn test-compile JDK 21 lỗi
      Lombok JCImport; JDK 11 + skip=false vẫn hỏng vì test khác gọi method private.)
      Test profile trên thanh: ▶ @Test / Run file test tạo profile (test-profiles.json chung nvim, thêm class/method)
      và chọn làm active; danh sách cấu hình có nhóm "Unit test" 🧪; Run/Debug trên thanh với profile test ->
      chạy như test (profile nvim chỉ có file+lnum -> tìm method gần dòng). 52 test Rust.

## Đang dở / việc tiếp theo
- Giai đoạn 0 bước 2: thêm --jvm-arg=-Xmx2G / -Xms256m vào jdtls_launcher.lua, đo lại.
- java-debug-model có thay đổi chưa commit (maven.lua gốc không pom, main_gutter click
  getmousepos, và thay đổi của phiên khác); app đang dùng bản local (java.lua, ghi chú TẠM).
- tests/run.sh devterm lỗi ở wezterm_mock.lua (phần rehydrate/restore_missing đang làm dở).

### 2026-09-30 - Gợi ý code
- Popup gợi ý hiện danh sách param (labelDetails.detail), chọn hàm tự thêm "()" và đặt con trỏ trong ngoặc.
- Parameter info: Ctrl+P, tự hiện khi gõ '(' hoặc ','; param đang nhập tô đậm; Esc để đóng.
- Phím gợi ý: Ctrl+Space, Ctrl+Shift+Space, Alt+/ (UI test completion_keys_reach_the_editor pass).

### 2026-09-30 - Settings, Ctrl+B, JSF
- Settings: font + cỡ chữ log (console Run/Debug/Test); JDK mặc định của project (maven-jdk.json, dùng cho Maven/build và Run/Debug/Test khi cấu hình không chọn JDK).
- Ctrl+B trên chỗ khai báo → danh sách chỗ dùng (như IntelliJ).
- jdtls chạy với -Dmaven.test.skip=false (pom có maven.test.skip=true làm m2e bỏ src/test khỏi classpath).
- Crate `jsf` (port jsf/el.lua, bean_index.lua, resolve.lua): Ctrl+B/gợi ý/hover trong .xhtml (EL → Java qua jdtls, include/template, composite tag), Find Usages Java → thêm chỗ dùng trong .xhtml. `chodilamide jsf check <dir> <file>`: 17/17 EL của hello.xhtml (sample-project) resolve đúng.
- Chưa port: diagnostics xhtml (EL sai, ajax id), `◇ N xhtml` lens, rename JSF, page map, generate CRUD/LazyModel, extract component.

### 2026-09-30 - Cây project, Java/Maven do app quản lý (build OK, test 34+ pass tuần tự, CHƯA kiểm tay trong UI)
- Bỏ nhóm "Open Editors" trong cây. Hiện file đang mở trong cây: nút ⊙ trên header cây, Alt+F1, chuột phải tab.
- toolchain::managed: JDK Temurin (8/11/17/21/25) + Maven 3.9.11 tải vào ~/.local/share/chodilamide/tools; tự tải khi thiếu
  (jdtls cần 21+, Maven lúc đọc model); JDK do app tải được ưu tiên hơn JDK của OS; runner::config::project_jdk().
  Cài đặt → "Java & Maven (toàn app)". Đã tải Maven thật vào thư mục tạm: mvn -v 3.9.11. Chưa tải thử JDK trong app.

## Kế hoạch tính năng IntelliJ tiếp theo (30/9, chờ người dùng chọn thứ tự)
- Đợt A (sửa code hằng ngày): Ctrl+/ comment, Ctrl+R replace, Ctrl+E recent files, Ctrl+Alt+B implementation,
  Ctrl+H type hierarchy, Ctrl+Alt+H call hierarchy, Alt+Insert generate, gập code, Shift+Shift search everywhere.
- Đợt B (Git): cây file tô màu theo trạng thái, gutter diff, Commit (Ctrl+K), Log, blame, đổi nhánh, xem diff.
- Đợt C (hiểu code): semantic tokens, inlay hints, TODO tool window, Local History, Problems cho cả project, bookmarks.
- Đợt D (Run/Debug): danh sách thread, Attach to Process/cổng, Spring Boot (profile, actuator), HTTP client .http,
  coverage.
- Đợt E (đóng gói): nhiều con trỏ, updater, Windows/macOS.

### 2026-09-30 - Spring Boot + JSF (build OK, test tuần tự pass; CHƯA kiểm tay trong UI)
- Crate mới `spring` (quét text, không cần jdtls): application.properties/yml (parse YAML phẳng hóa, list, block
  scalar, multi-doc), profiles (application-<p>.* + spring.config.activate.on-profile), `${…}` trong Java, relaxed
  binding (mail-host = mailHost), metadata Spring Boot từ jar (META-INF/spring-configuration-metadata.json, cache theo
  mtime) + target/classes, Endpoints (@RequestMapping lớp × @Get/Post/…Mapping, method=, hằng → {NAME}, bỏ comment,
  bỏ @FeignClient), Beans (@Component/@Service/…/@Bean, kiểu inject được: class + extends/implements).
  CLI `chodilamide spring <dir> [key]`: cloud-service-management 112 endpoint 16ms, 327 bean, profiles dev/prod,
  2376 key metadata 49ms; key ECM.CLOUD.URL → 5 nơi đặt + @Value ở ECMCloudClient.
- App: gợi ý key trong properties/yml (yml: theo section cha, chèn "rel: ") và trong @Value("${"; gợi ý giá trị (hints,
  boolean, log level); Ctrl+B key → code Java đọc nó / ${key} → nơi đặt; Ctrl+Q mô tả + giá trị theo từng profile;
  Alt+8 Endpoints; "Spring: danh sách Beans"; Ctrl+Alt+Shift+B tới bean được inject (List<X> → X); chip Spring
  profiles trong dialog cấu hình chạy (bật/tắt -Dspring.profiles.active). UI test: gõ "server.po" → chọn → Ctrl+B.
- JSF (port nốt jsf/*.lua): kiểm tra .xhtml (ajax id không có, @all, search expression lạ, c:forEach/c:if, dataTable
  paginator không lazy, EL không resolve qua jdtls - chỉ "không tìm thấy") gạch lượn khi mở/lưu/jdtls sẵn sàng; hint
  vẽ xám. CLI jsf check: trang mẫu 0 lỗi ~190ms, bản sai 2 lỗi (kể cả qua biến var=). Rename Shift+F6 trong bean:
  đổi luôn field/getter/setter còn lại qua jdtls + mọi #{bean.x}/#{bean['x']} trong .xhtml (tab mở sửa buffer, file
  đóng ghi đĩa). Lens "◇ N xhtml" cuối dòng getter/action. Page Map. Generate CRUD/LazyDataModel từ @Entity (repo
  dùng lại nếu có, javax/jakarta theo pom), Extract Component từ vùng chọn. Menu chuột phải có các mục này.

### 2026-09-30 - Đợt A: sửa code hằng ngày (build OK, test tuần tự pass; CHƯA kiểm tay)
- editor-core: Ctrl+/ comment dòng (// # <!-- --> theo loại file, bỏ comment khi mọi dòng đã comment, xuống dòng kế),
  Ctrl+Shift+/ comment khối, Ctrl+Shift+↑/↓ và Alt+Shift+↑/↓ chuyển dòng, Tab/Shift+Tab thụt lề cả vùng chọn,
  Ctrl+Shift+J nối dòng, Ctrl+Shift+U HOA/thường, Extend/Shrink Selection (Find Action - Ctrl+W vẫn là đóng tab).
- Gập code: vùng theo thụt lề + khối import; Ctrl+- / Ctrl+= / Ctrl+Shift+- / Ctrl+Shift+=; "⋯" sau dòng đầu
  (bấm để mở); vùng gập sống trong Buffer::apply (dịch theo sửa, bỏ khi sửa vào trong); con trỏ nhảy vào thì mở;
  editor vẽ theo hàng hiển thị.
- Ctrl+R thay trong file (Enter thay+tiếp, Alt+A tất cả, 1 undo), Ctrl+Shift+R thay trong project (xem trước số chỗ,
  tab mở sửa buffer, file khác ghi đĩa + báo jdtls), Ctrl+E Recent Files, Shift Shift Search Everywhere (symbol jdtls
  + file + action).
- jdtls: Ctrl+Alt+B implementation, Ctrl+U super, Ctrl+H type hierarchy (↑ cha xa nhất trước, ↓ con 4 cấp), Ctrl+Alt+H
  call hierarchy (3 cấp), Alt+Insert Generate: bật prompt support (toString/equals-hashCode/constructors/accessors/
  override) và app tự trả lời prompt (chọn tất cả field). CLI `lsp refs`: HelloBean supertypes BaseBean, toString 12 field.

### 2026-09-30 - Đợt B–E (build OK, test tuần tự: app 49, editor-core 13, jsf 16, spring 6, vcs 3, DAP 17/17,
attach_pause_test.py 4/4; CHƯA ai bấm thử trong UI)
- Git (crate `vcs`, gọi git CLI; diff Myers tự viết so buffer chưa lưu với HEAD): màu file trong cây (M xanh dương,
  A xanh lá, U đỏ, conflict đỏ), vạch gutter thêm/sửa/xóa, Ctrl+Alt+Z / menu rollback khối, nhánh "⎇ main (n)" ở status
  bar → menu (commit, push, update, log, diff, nhánh mới, checkout local/remote), Ctrl+K Commit (chọn file, bấm đúp xem
  diff, amend, Commit & Push; tự lưu file trước), Ctrl+Shift+K push (tự -u lần đầu), Ctrl+T pull --ff-only, Alt+9 Log
  → Enter mở `git show` read-only, History file, So sánh với HEAD, Annotate (cột blame ở gutter), Alt+` menu Git.
  Làm mới: mở project, lưu file, watcher, mỗi 30s. UI test trên repo thật: vạch sửa → rollback → Ctrl+K commit.
- TODO tool window, Bookmarks (F11 / Shift+F11, lưu chodilamide-bookmarks.json), Local History (mỗi lần lưu, 60 bản/file ở
  ~/.local/share/chodilamide/history, xem read-only, "Khôi phục phiên bản này").
- Semantic highlighting từ jdtls (field tím, static nghiêng, annotation vàng, deprecated gạch) - jdtls trả 52 token cho
  HelloBean. KHÔNG làm inlay hints (cần chữ ảo giữa dòng trong layout editor).
- Debug: Pause (⏸; debugger chọn thread main khi không có threadId), danh sách Threads bấm để xem stack, Attach vào JVM
  (host:port, Remote JVM Debug). HTTP Client .http/.rest (###, {{var}} từ http-client.env.json env dev/local, ▶ gutter /
  Ctrl+Enter, curl, JSON đẹp) - UI test với server thật. Run with Coverage (JaCoCo 0.8.12 tự tải/copy từ ~/.m2, agent vào
  test JVM, jacococli report XML → vạch xanh/vàng/đỏ + % trong tab Run) - test thật: Greeter 6/6, Main 11/12.
- Nhiều con trỏ: Alt+J / Alt+Shift+J / Ctrl+Alt+Shift+J, Alt+Click; gõ/Backspace/Delete/Enter/dán cho mọi con trỏ, 1 undo
  (Buffer::begin_compound); IME preedit chỉ con trỏ chính. Ctrl+N mở được class trong jar (jdt://).
- CHƯA làm: inlay hints, updater, gói Windows/macOS (không kiểm được tại chỗ), tài liệu trong popup gợi ý, Spring
  actuator. testrunner_test.py cần project JUnit mẫu (thư mục cũ đã mất) - chưa chạy lại.

### 2026-09-30 - Vim + phím tắt (build OK, test tuần tự pass; CHƯA kiểm tay, CHƯA thử với fcitx5 thật)
- Sửa: gợi ý EL trong .xhtml tự bật khi gõ "#{bean." (trả lời của yêu cầu cũ bị nhận nhầm - nay so vị trí jsf_at).
  Thêm gợi ý thẻ/attribute JSF: taglib *.taglib.xml đọc từ jar (PrimeFaces 13: 184 thẻ) + h: f: ui: c: composite: có sẵn,
  attribute chèn name="" con trỏ giữa nháy. Test jdtls thật (#[ignore] jsf_member_completion_real_jdtls).
- Vim (editor_core::vim, 19 test): Normal/Insert/Visual/V-Line, count, h j k l w b e W B E 0 ^ $ gg G { } % f F t T ; ,
  C-d C-u, d c y > < g~ gu gU với motion/text object (iw aw i" a" i( a( i{ it at ip…), dd cc yy >> <<, x X s S D C Y p P r
  J ~ u C-r ., register "a–"z "0, clipboard, m/`', / ? n N * #, gd gr gi gD K ]d [d gcc za zc zo zM zR gt gT C-o C-i,
  ':' w q wq x qa e N s///g %s///g noh bn bp. vimrc ~/.config/chodilamide/vimrc: let mapleader, (n|v|x|i)(nore)map,
  <Action>(tên) / :action tên<CR>, set clipboard; lưu là nạp lại.
- App: Cài đặt → Phím tắt: Kiểu phím IntelliJ/Vim, chia phím Ctrl Vim/IDE (mặc định Vim: C-r C-d C-u C-o C-i C-w C-[),
  nút Sửa vimrc (tạo mẫu), nút Phím tắt app (Find Action). Phím chặn trước binding IDE (intercept_keystrokes) chỉ khi
  editor có focus; Normal/Visual không đăng ký input handler (fcitx không ghép "dd"→"đ"); con trỏ khối; "-- NORMAL --".
- Thêm (sau khi reset): Visual Block C-v (d x c s I A y r o > <, mỗi dòng 1 con trỏ, Esc về góc trên-trái), macro
  q{reg}…q / @{reg} / @@ (ghi cả chữ gõ ở Insert), text object trong Visual (viw vi( vat…), gc{motion} gcip và gc trong
  Visual. C-v mặc định thuộc Vim. Test: editor-core 20, UI test vim_mode_keys (block insert + macro qa…q @a).

### 2026-10-01 - Gói Arch 0.2.0
- depends chỉ còn curl tar vulkan-icd-loader libxkbcommon wayland fontconfig; JDK/Maven/git thành optdepends (app tự tải
  JDK 21, Maven 3.9.11; mới: jdtls + lombok qua managed::ensure_jdtls khi mở project lần đầu - test máy trống: 7.5s).
- makepkg OK: packaging/arch/chodilamide-ide-0.2.0-1-x86_64.pkg.tar.zst (11.6 MB). Giải nén thử + HOME trống: jar
  debugger/testrunner/agent tìm thấy qua /usr/share/chodilamide, ldd không thiếu thư viện. CHƯA pacman -U (cần sudo).

### 2026-10-01 - Windows (dừng giữa chừng, người dùng chọn "để sau")
- `cargo build --release --target x86_64-pc-windows-gnu -p chodilamide-app` biên dịch được toàn bộ code app (sửa
  project-model: mtime không dùng MetadataExt, is_executable cfg(unix), mvnw.cmd trên Windows).
- CHƯA có .exe chạy được: gpui build.rs chỉ biên dịch shader HLSL khi máy build là Windows (fxc.exe). Đã thử với
  shaders_bytes.rs giả (mảng rỗng) để kiểm biên dịch - bản đó KHÔNG vẽ được, đã xóa exe. Lựa chọn còn lại: GitHub Actions
  windows-latest (khuyên dùng, cần đẩy code lên repo private), Wine + fxc.exe (NuGet Windows SDK BuildTools), vkd3d.
- Gói cũ `chodilamide` 0.1.0 còn trên máy: người dùng tự `sudo pacman -Rns chodilamide`.

### 2026-10-01 - Git: cửa sổ Git, diff hai cột, xung đột (build OK, test tuần tự pass; CHƯA kiểm tay)
- vcs: changes (XY: staged/unstaged/untracked/conflict), ahead/behind, repo_state (merge/rebase/cherry-pick/revert) +
  continue/abort/skip (GIT_EDITOR=true), stage/unstage/discard, stash list/push/apply/pop/drop, merge/rebase, file_at(rev),
  side_by_side (cột trái/phải căn dòng), parse_conflicts (cả diff3), resolve_conflict ours/theirs/both, take_side,
  mark_resolved. pull = `git pull --autostash` (lỗi cũ: pull.rebase=true + file sửa dở → git từ chối).
- App: tab Git trong panel dưới (Alt+9; "Git ⚠" khi có xung đột): nhánh ↑↓, banner ĐANG MERGE/REBASE + Continue/Skip/
  Abort, nhóm Xung đột / Đã stage / Thay đổi / Chưa theo dõi với So sánh, +Stage, −Unstage, ↺ Hủy (hỏi xác nhận), Stash…,
  danh sách stash Áp dụng/Pop/Xóa. Diff hai cột (uniform_list, F7/Shift+F7 thay đổi kế, Esc) từ panel, Commit (bấm đúp),
  menu editor/cây "So sánh với HEAD" và "So sánh với nhánh / commit…". Trình giải quyết xung đột: từng khối Của bạn ↔
  Của nhánh kia, Lấy của bạn / Lấy của nhánh kia / Lấy cả hai, Toàn bộ của bạn/nhánh kia, Sửa tay, Đánh dấu đã giải quyết.
  Menu nhánh: Merge nhánh khác vào đây…, Rebase lên…; xung đột → tự mở cửa sổ Git. UI test repo thật: xung đột → theirs →
  resolved → continue; diff rows Same/Changed/Added/Same.
- Debug unit test chạy code cũ: lưu mọi file trước khi Run/Debug + class_file_fresh (.class phải mới hơn .java, cũ →
  javac lại). UI test run_uses_new_test_code.

### 2026-10-01 - Git làm lại theo SmartGit (build OK, 0 warning; test tuần tự: app 57, vcs 10 pass; CHƯA kiểm tay)
- Sửa lỗi: commit = commit INDEX (bỏ pathspec) → commit được khi đang merge, rename không còn treo phần xóa file cũ,
  stage từng phần có tác dụng; amend message trống = giữ message cũ. Checkout nhánh remote không còn `checkout -B`
  (trước đây reset nhánh local, mất commit chưa push). Lấy cả file một bên với DU/UD/DD = `git rm`. Discard file
  rename trả lại tên cũ. Marker xung đột phải đúng 7 ký tự (dòng `=======` trong markdown không còn bị hiểu nhầm).
  Push: remote mặc định không cố định "origin", báo rõ khi bị từ chối (non-fast-forward), thêm force-with-lease.
- State: mọi lệnh git chạy nền qua `run_git` (một lệnh mỗi lúc, "⏳ …" trên panel), refresh có số thứ tự (kết quả cũ
  về muộn bị bỏ), chỉ một `git status` mỗi lần refresh, danh sách nhánh/remotes lấy từ cache.
- Cửa sổ Git: danh sách file + ô commit chung một chỗ (bỏ hộp Commit modal; Ctrl+K mở cửa sổ và focus message).
  Chọn nhiều (Ctrl/Shift-click, ↑↓, Shift+↑↓), ô ☐/☑ = stage/unstage, Space/Enter/Delete, menu chuột phải, nhóm theo
  thư mục, rename hiện `cũ → mới`, stash có/không file chưa theo dõi.
- Diff: theo nhóm (Thay đổi = index ↔ work tree, Đã stage = HEAD ↔ index, menu = rev ↔ hiện tại), tô ký tự khác trong
  dòng, Stage/Unstage/Hoàn tác từng khối (Ctrl+Alt+A / Ctrl+Alt+Z; ghi index trực tiếp bằng hash-object +
  update-index), báo "chỉ khác CRLF/LF", tải nền.
- Xung đột: 3 cột trái / bản gốc / phải (base lấy từ diff3 hoặc `git merge-file --diff3` trên stage :1:2:3), pane Kết
  quả cả file, F7/Shift+F7, Hoàn tác, Lấy bản gốc, Cả hai theo 2 thứ tự, nhãn đúng khi rebase (ours/theirs đảo).
- Log (crate vcs/work.rs + app/git_log.rs): graph lane (--topo-order, mọi nhánh / nhánh hiện tại / history file),
  nhãn HEAD/nhánh/remote/tag, lọc, pane chi tiết + file đổi (bấm = diff), menu: checkout nhánh/commit, nhánh/tag mới,
  cherry-pick, revert, merge, rebase, reset soft/mixed/hard (hỏi), xóa (hỏi lại nếu chưa merge) / đổi tên nhánh, copy hash.
- Chưa làm: interactive rebase, sửa trực tiếp trong cột phải của diff, stage từng dòng (mới có từng khối).
- Bản sao trước khi sửa: scratchpad phiên Claude (crates/ cũ) - repo chưa git init.

### 2026-10-01 - Giao diện kiểu IntelliJ New UI / Islands (build OK, test tuần tự pass; tự kiểm bằng ảnh chụp Dracula + Light)
- Main toolbar một hàng (thay hàng tab project + toolbar cũ): project widget (ô chữ tắt màu theo tên + tên, ×), +,
  VCS widget (icon nhánh, số file đổi, ▾) + Pull/Commit/Push dạng icon, bên phải run widget (cấu hình ▾, ▶, bug, ■),
  tìm mọi nơi, cài đặt. Tooltip cho mọi nút icon (ui_icons::tip).
- Icon vector tự vẽ (crate app/src/ui_icons.rs, PathBuilder 16×16, nét 1.5px) thay emoji/ký tự.
- Thanh tool window dọc (40px, phía cây): Project, Git, Maven | Run/Debug, Terminal, Problems; nút đang mở tô nền.
- Islands: nền khung FRAME_BG (tự suy ra cho 12 theme), editor / cây / panel là khối bo 8px cách nhau 5px (khe =
  thanh kéo đổi kích thước), bỏ viền kẻ. Header tool window: tên in đậm + nút icon (định vị, file ẩn, ẩn).
- Tab editor cao 36px không vạch ngăn, tab đang mở gạch chân accent, file chưa lưu = chấm (thành × khi đang mở).
  Cây: dòng 24px, chọn bo góc, mũi tên chevron. Status bar trên nền khung, đường dẫn dạng breadcrumb ›.
- Chụp kiểm: CHODILAMIDE_OPEN=git mở panel Git.

### 2026-10-01 - Bản Windows build được trên Linux (dist/ChoDiLamIDE-0.2.0-windows-x64.zip; CHƯA chạy trên Windows thật)
- packaging/windows/build.sh: cross-compile x86_64-pc-windows-gnu, zip gồm ChoDiLamIDE.exe, chodilamide-cli.exe, 3 jar,
  README.txt. Exe chỉ cần DLL hệ thống của Windows 10/11 (UCRT, d3d11, dwrite…), không cần DLL mingw.
- Shader HLSL của GPUI: vendor/gpui (patch.crates-io) - build.rs chạy bước Windows theo TARGET (gốc: theo máy build),
  embed-resource luôn là build-dep (manifest qua mingw windres). GPUI_FXC_PATH=packaging/windows/fxc/fxc-wine:
  fxc.exe tự viết (fxc.c, mingw) gọi d3dcompiler_47.dll thật của Microsoft (NuGet Microsoft.Windows.SDK.CPP) qua Wine.
  16/16 shader ra DXBC. vkd3d-compiler thử trước: lỗi SV_ClipDistance ở 7 shader - bỏ.
- Sửa cho Windows (cũng có lợi cho Linux): jdtls khởi động thẳng bằng java + equinox launcher (không còn bin/jdtls
  Python; config_win/config_linux/config_mac theo OS) - kiểm trên máy giả lập Linux: ServiceReady 2.1s; mọi
  đường dẫn HOME lùi về USERPROFILE; JetBrains Mono nhúng trong exe (cx.text_system().add_fonts, chỉ cfg(windows)).
- Thử dưới Wine 11.18: exe khởi động, D3D11 + shader nạp được; Wine thiếu DirectComposition (né bằng
  GPUI_DISABLE_DIRECT_COMPOSITION=1) và DirectWrite của Wine không tìm thấy font nào (kể cả Tahoma) -> không vẽ chữ được
  trong Wine, giới hạn của Wine. CLI doctor dưới Wine: tìm thấy jar cạnh exe.
- Còn phải kiểm trên Windows thật: mở cửa sổ, font, tự tải JDK/Maven/jdtls (curl.exe/tar.exe có sẵn từ Win10 1803),
  Run/Debug/Test, terminal (ConPTY), Git.
