ChoDiLamIDE cho Windows (bản thử nghiệm)
========================================

Yêu cầu
- Windows 10 (1903 trở lên) hoặc Windows 11, 64-bit, card đồ họa có DirectX 11.
- Mạng Internet lần đầu mở project: app tự tải JDK 21, Maven, jdtls, lombok vào
  %USERPROFILE%\.local\share\chodilamide\tools (không cần cài Java / Maven trước).
- Git cho Windows (https://git-scm.com) nếu muốn dùng Commit / Push / Log.

Cài
1. Giải nén cả thư mục ChoDiLamIDE vào nơi cố định, ví dụ C:\Tools\ChoDiLamIDE
   (các file .jar phải nằm cạnh ChoDiLamIDE.exe).
2. Chạy ChoDiLamIDE.exe. Lần đầu Windows SmartScreen có thể cảnh báo vì file chưa ký:
   bấm "More info" -> "Run anyway".
3. Chọn project (nút "Chọn thư mục…" hoặc gõ đường dẫn).

Project dùng Nexus / repo Maven nội bộ: đặt settings.xml của công ty vào
%USERPROFILE%\.m2\settings.xml như khi dùng Maven bình thường.

Kiểm tra môi trường: mở cmd trong thư mục này, chạy
    chodilamide-cli.exe doctor

Font JetBrains Mono đã nằm trong ChoDiLamIDE.exe (giấy phép: JetBrainsMono-OFL.txt).
