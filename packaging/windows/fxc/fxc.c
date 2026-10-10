/* Minimal fxc.exe stand-in: the arguments gpui's build.rs passes
   (/T target /E entry /Fh out.h /Vn name /O3 file.hlsl), compiled by Microsoft's
   d3dcompiler_47.dll (D3DCompileFromFile), output in fxc's /Fh header format. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef struct ID3DBlobVtbl {
    void *QueryInterface, *AddRef;
    ULONG (__stdcall *Release)(void *);
    void *(__stdcall *GetBufferPointer)(void *);
    SIZE_T (__stdcall *GetBufferSize)(void *);
} ID3DBlobVtbl;
typedef struct { ID3DBlobVtbl *v; } Blob;
typedef HRESULT (__stdcall *CompileFn)(LPCWSTR, const void *, void *, LPCSTR, LPCSTR, UINT, UINT, Blob **, Blob **);
#define D3DCOMPILE_OPTIMIZATION_LEVEL3 (1 << 15)
#define STANDARD_INCLUDE ((void *)(UINT_PTR)1)

int main(int argc, char **argv) {
    const char *target = 0, *entry = 0, *out = 0, *name = 0, *file = 0;
    UINT flags = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "/T") && i + 1 < argc) target = argv[++i];
        else if (!strcmp(argv[i], "/E") && i + 1 < argc) entry = argv[++i];
        else if (!strcmp(argv[i], "/Fh") && i + 1 < argc) out = argv[++i];
        else if (!strcmp(argv[i], "/Vn") && i + 1 < argc) name = argv[++i];
        else if (!strcmp(argv[i], "/O3")) flags |= D3DCOMPILE_OPTIMIZATION_LEVEL3;
        else file = argv[i];
    }
    if (!target || !entry || !out || !name || !file) { fprintf(stderr, "usage: fxc /T t /E e /Fh out /Vn name [/O3] file\n"); return 2; }
    HMODULE dll = LoadLibraryA("d3dcompiler_47.dll");
    if (!dll) { fprintf(stderr, "d3dcompiler_47.dll not found\n"); return 3; }
    CompileFn compile = (CompileFn)GetProcAddress(dll, "D3DCompileFromFile");
    WCHAR wfile[MAX_PATH * 2];
    MultiByteToWideChar(CP_UTF8, 0, file, -1, wfile, MAX_PATH * 2);
    Blob *code = 0, *err = 0;
    HRESULT hr = compile(wfile, 0, STANDARD_INCLUDE, entry, target, flags, 0, &code, &err);
    if (err) fprintf(stderr, "%s\n", (char *)err->v->GetBufferPointer(err));
    if (FAILED(hr) || !code) { fprintf(stderr, "compilation failed: 0x%08lx\n", (unsigned long)hr); return 1; }
    const unsigned char *b = code->v->GetBufferPointer(code);
    SIZE_T n = code->v->GetBufferSize(code);
    FILE *f = fopen(out, "wb");
    if (!f) { fprintf(stderr, "cannot write %s\n", out); return 4; }
    fprintf(f, "const BYTE %s[] =\n{\n", name);
    for (SIZE_T i = 0; i < n; i++) fprintf(f, "%s0x%02x%s", i % 16 ? " " : "    ", b[i], i + 1 < n ? (i % 16 == 15 ? ",\n" : ",") : "\n");
    fprintf(f, "};\n");
    fclose(f);
    return 0;
}
