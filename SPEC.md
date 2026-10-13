# Spec: Session Manager untuk Claude Code

> Nama kerja: **codemux** (repo `codemux`). Di dokumen ini juga disebut **"app"**.
> Jangan memakai kata "Claude" di nama app (merek Anthropic). Pakai "for Claude Code" di deskripsi.

Status: draf v0.2 · 2026-10-13 (tambah "Terminal baru")

---

## 1. Masalah

- Dalam sehari bisa ada 5–10 session Claude Code, masing-masing di tab tmux (di Ghostty).
- Setelah restart macOS, semua tab tmux hilang.
- Besoknya sulit ingat session apa saja yang dibuka dan apa isinya.

Data session sebenarnya tidak hilang. Claude Code menyimpan setiap session di
`~/.claude/projects/<folder>/<session-id>.jsonl`, dan session bisa dibuka lagi dengan
`claude --resume <session-id>`. Yang hilang hanya **terminal dan susunan tab-nya**.

## 2. Tujuan

App desktop dengan **satu window**: sidebar daftar session di kiri (seperti Claude/ChatGPT),
terminal Claude Code di kanan.

1. Semua session tampil di sidebar: yang sedang aktif dan yang lama.
2. Indikator jelas: mana yang sedang bekerja, mana yang menunggu input, mana yang lama.
3. Terminal Claude Code berjalan **di dalam app**.
4. Setelah restart OS, session yang tadinya terbuka muncul lagi, dan bisa dilanjutkan dengan satu klik.
5. Jalan di **macOS, Windows, dan Linux** dari satu codebase.
6. Open source.

## 3. Bukan tujuan (v1)

- Bukan pengganti terminal umum. Tab "Terminal baru" (bagian 11.5) memang berisi shell biasa,
  tapi tujuannya hanya untuk memulai Claude Code. Tidak ada fitur terminal lanjutan (split, profil, dsb).
- Tidak mengubah atau mengirim file session Claude Code ke mana pun. Menghapus hanya atas perintah user
  (bagian 11.6).
- Tidak memanggil API Claude/Anthropic secara langsung.
- Tidak mendukung session di dalam WSL (lihat bagian 14).
- Tidak ada sinkronisasi antar komputer.

## 4. Keputusan teknis

| Topik | Keputusan | Alasan |
|---|---|---|
| Bahasa | Rust | Satu codebase untuk 3 OS, cepat, aman |
| UI framework | **Iced 0.14** (versi dikunci) | Bisa membuat widget rendah (terminal) dengan GPU, ada dukungan IME, terbukti dipakai COSMIC Terminal |
| Emulator terminal | **`alacritty_terminal` 0.26** | Matang, dirawat tim Alacritty, Apache-2.0 |
| PTY | `alacritty_terminal::tty` (ConPTY di Windows) | Sudah satu paket dengan emulator |
| Widget terminal | **Crate sendiri** `crates/terminal`, diawali dari kode `iced_term` 0.8 (MIT) | Terminal adalah inti app. Kode dipegang sendiri, bukan bergantung pada proyek kecil |
| Bukan Tauri | — | Permintaan pemilik proyek |
| Tampilan | Tema sendiri yang rapi dan konsisten di 3 OS, memakai font sistem | Terminal memakai ~80% layar dan memang tidak pernah "asli" di app mana pun |

## 5. Pengguna dan skenario

**Pengguna:** developer yang menjalankan banyak session Claude Code setiap hari.

**Skenario utama:**

1. **Pagi hari setelah restart.** Buka app. Sidebar menampilkan "Terbuka sebelumnya" (7 session kemarin)
   dan riwayat lain. Klik satu session → terminal muncul dan menjalankan `claude --resume <id>`.
2. **Mulai session baru.** Tekan ⌘N / Ctrl+Shift+N → pilih folder proyek → session baru jalan di tab baru.
2b. **Terminal baru.** Tekan ⌘T / Ctrl+Shift+T → shell biasa terbuka. User `cd` ke folder mana saja lalu
   mengetik `claude` sendiri. App mendeteksinya, dan tab itu otomatis menjadi session di sidebar.
   Begitu Claude Code menulis `ai-title`, judul tab ikut berubah.
3. **Berpindah session.** Klik session lain di sidebar. Session sebelumnya tetap berjalan di belakang.
4. **Tahu mana yang butuh perhatian.** Session yang menunggu izin atau jawaban diberi tanda kuning,
   dan (opsional) memunculkan notifikasi OS.
5. **Mencari session lama.** Ketik di kotak cari → cocokkan judul, folder, branch, dan prompt terakhir.

## 6. Tampilan (UI)

```
┌──────────────────────┬─────────────────────────────────────────┐
│ 🔍 Cari session...    │  fix webhook retry          qismo-api   │
│ [+ Session][>_ Term]  │  fix/webhook · 2 jam lalu    [■ Stop]   │
│                      ├─────────────────────────────────────────┤
│ AKTIF                 │                                         │
│ ● fix webhook retry   │        (terminal Claude Code)           │
│   qismo-api · 2m      │                                         │
│ ◐ review PR #812      │                                         │
│   sdk-api · menunggu  │                                         │
│                      │                                         │
│ TERBUKA SEBELUMNYA    │                                         │
│ ○ aplikasi GUI macOS  │                                         │
│                      │                                         │
│ HARI INI / KEMARIN /  │                                         │
│ 7 HARI / LEBIH LAMA   │                                         │
└──────────────────────┴─────────────────────────────────────────┘
```

### 6.1 Sidebar

- Kelompok, dari atas ke bawah:
  1. **Disematkan** (pin)
  2. **Aktif**: punya terminal yang berjalan di app (termasuk tab "Terminal baru", walau belum menjalankan
     Claude), atau terdeteksi berjalan di luar app
  3. **Terbuka sebelumnya**: tadinya terbuka saat app ditutup atau OS restart
  4. **Hari ini**, **Kemarin**, **7 hari terakhir**, **30 hari terakhir**, **Lebih lama**
     (berdasarkan waktu aktivitas terakhir)
- Isi setiap baris:
  - Indikator status (bagian 7)
  - Judul (bagian 8.3)
  - Baris kecil: nama folder proyek · branch · waktu relatif ("2m", "08:10", "kemarin")
- Menu klik kanan: Buka, Pin/Lepas pin, Ganti nama (hanya label lokal di app), Arsipkan,
  Salin session ID, Buka folder, Hentikan, Hapus (bagian 11.6).
- Session yang diarsipkan disembunyikan, tapi bisa ditampilkan lewat filter.

### 6.2 Area kanan

- Header: judul, folder, branch, waktu terakhir, tombol **Stop**.
- Terminal: penuh, dan ikut berubah ukuran mengikuti window.
- Kalau session belum dijalankan: tampilkan ringkasan (judul, prompt terakhir, folder) dan tombol
  **Lanjutkan** (= `claude --resume`).

### 6.3 Shortcut

| Aksi | macOS | Windows/Linux |
|---|---|---|
| Session baru (pilih folder) | ⌘N | Ctrl+Shift+N |
| Terminal baru (shell biasa) | ⌘T | Ctrl+Shift+T |
| Tutup tab | ⌘W | Ctrl+Shift+W |
| Cari | ⌘K | Ctrl+K |
| Pindah ke session 1–9 di grup Aktif | ⌘1–⌘9 | Ctrl+1–9 |
| Session berikut/sebelumnya | ⌘⇧] / ⌘⇧[ | Ctrl+Tab / Ctrl+Shift+Tab |
| Sembunyikan/tampilkan sidebar | ⌘\ | Ctrl+\ |

Di Windows/Linux, shortcut app memakai Ctrl+Shift, karena Ctrl+N, Ctrl+T, dan Ctrl+W dipakai oleh shell dan Claude Code.

Shortcut app tidak boleh menutupi shortcut yang dipakai Claude Code di terminal
(misalnya Ctrl+C, Ctrl+R, Shift+Tab, Esc). Kalau terjadi bentrok, terminal yang diutamakan.

### 6.4 Tema

- Ikut mode terang/gelap OS.
- Font UI: font sistem (SF Pro / Segoe UI / font default distro Linux).
- Font terminal: bisa diatur, default font monospace sistem.

## 7. Status session

| Ikon | Status | Arti | Sumber data |
|---|---|---|---|
| ● hijau | **Bekerja** | Proses berjalan di app, dan Claude sedang memproses | Hook (bagian 9), cadangannya output PTY baru dalam 2 detik terakhir |
| ◐ kuning | **Menunggu input** | Proses berjalan, Claude menunggu izin atau jawaban | Hook `Notification` / `Stop` |
| ◉ biru | **Aktif di luar app** | Session berjalan di terminal lain | File `.jsonl` berubah dalam 2 menit terakhir **dan** ada proses `claude` dengan `cwd` yang sama |
| ▸ | **Shell** | Tab "Terminal baru" yang belum menjalankan Claude | Proses anak shell (bagian 11.5) |
| ○ abu-abu | **Tidak berjalan** | Session lama, bisa dilanjutkan | Default |
| ✕ merah | **Keluar dengan error** | Proses di app berhenti dengan kode bukan 0 | Exit code PTY |

Aturan:
- Status dari hook diutamakan. Kalau hook tidak dipasang, pakai cadangan (output PTY dan perubahan file).
- Session "aktif di luar app" **tidak boleh** dibuka lagi di app sebelum proses luarnya berhenti,
  supaya satu session tidak berjalan dua kali. Tombol Lanjutkan diganti peringatan.

## 8. Sumber data: file session Claude Code

### 8.1 Lokasi

| OS | Path |
|---|---|
| macOS / Linux | `~/.claude/projects/` |
| Windows | `%USERPROFILE%\.claude\projects\` |

Kalau variabel `CLAUDE_CONFIG_DIR` diset, pakai folder itu.

### 8.2 Field yang dipakai

Setiap baris file `.jsonl` adalah satu objek JSON dengan field `type`. Yang dipakai:

| `type` | Field | Untuk |
|---|---|---|
| `user` / `assistant` | `sessionId`, `cwd`, `gitBranch`, `timestamp` | Folder, branch, waktu |
| `user` (bukan `isMeta`, bukan `isSidechain`) | `message` | Prompt pertama (cadangan judul) |
| `ai-title` | `aiTitle` | Judul otomatis |
| `agent-name` | `agentName` | Nama yang diberi user (`claude -n`) |
| `last-prompt` | `lastPrompt` | Teks kecil di sidebar dan pencarian |
| `continued-in` | `continuedInSessionId` | Menggabungkan session yang dilanjutkan ke ID baru |

### 8.3 Urutan judul

1. Nama lokal di app (kalau user mengganti nama)
2. `agentName`
3. `aiTitle` terakhir
4. Prompt pertama user (dipotong 80 karakter)
5. "(tanpa judul)"

### 8.4 Aturan parser

- **Read-only**, dengan satu pengecualian: menghapus session atas perintah user (bagian 11.6).
  App tidak pernah mengubah isi file session.
- Format ini **bukan API resmi** dan bisa berubah. Karena itu:
  - Parser ada di satu modul (`core::sessions::parse`), dengan test dari file contoh (fixture).
  - Baris yang rusak atau `type` yang tidak dikenal dilewati, bukan error.
  - Field yang hilang tidak membuat app crash.
- Untuk file besar, baca secara **bertahap**: simpan posisi terakhir (offset) per file, dan baca
  hanya baris baru saat file berubah.
- Lewati file session milik subagent (sidechain).
- Index disimpan di cache (bagian 10), supaya app cepat terbuka.

### 8.5 Memantau perubahan

- Crate `notify` (FSEvents / ReadDirectoryChangesW / inotify).
- Event digabung tiap 250 ms (debounce), supaya tidak memproses terlalu sering.

## 9. Hook Claude Code (opsional, untuk status akurat)

- App **menawarkan** untuk memasang hook. App tidak memasangnya diam-diam.
- Sebelum dipasang, tampilkan perubahan yang akan ditulis ke `~/.claude/settings.json`, lalu minta persetujuan.
- Event: `SessionStart` → hubungkan tab dengan session, `UserPromptSubmit` → bekerja, `Notification` → menunggu input, `Stop` → menunggu input/selesai.
- Setiap PTY yang dibuat app diberi env `CODEMUX_TAB_ID=<id>`. Proses `claude` di tab itu mewarisinya,
  jadi hook bisa menulis `{tab_id, session_id, cwd}`. Ini cara paling akurat untuk tab "Terminal baru",
  dan juga menangani `/clear` atau `/resume` di dalam Claude yang mengganti ID session.
- Hook menulis file kecil ke folder data app: `status/<session-id>.json` berisi `{state, ts}`.
- Hook harus sangat ringan (tanpa jaringan), dan aman kalau app tidak sedang berjalan.
- Ada tombol "Lepas hook", yang mengembalikan `settings.json` seperti semula.

## 10. Data milik app

Disimpan di folder data per OS (crate `directories`):
- macOS: `~/Library/Application Support/<app>/`
- Windows: `%APPDATA%\<app>\`
- Linux: `~/.local/share/<app>/`

| File | Isi |
|---|---|
| `state.json` | Daftar session yang terbuka (urutan tab), session yang dipilih, ukuran window, lebar sidebar |
| `sessions.json` | Per session: pin, arsip, nama lokal, catatan |
| `index-cache.json` | Cache hasil index (bisa dihapus kapan saja) |
| `status/` | File status dari hook |

- Simpan secara **atomic** (tulis ke file sementara, lalu rename), supaya file tidak rusak kalau app mati mendadak.
- Ada field `version` di setiap file, untuk migrasi format di versi berikutnya.

## 11. Siklus hidup session

### 11.1 Melanjutkan session (resume)

1. User klik session dengan status ○.
2. App membuat PTY dengan ukuran sesuai area terminal.
3. Perintah dijalankan **lewat login shell user**, supaya `PATH` sama dengan di terminal biasa:
   - macOS/Linux: `$SHELL -l -c 'exec claude --resume <id>'`, dengan `cwd` = folder session
   - Windows: `claude.exe --resume <id>` (cari lewat `PATH`), dengan `cwd` = folder session
4. Environment:
   - Set `TERM=xterm-256color` dan `COLORTERM=truecolor`
   - **Hapus** variabel `CLAUDE_CODE_*` yang diwarisi dari proses induk.
     (Dari tes: variabel penanda session yang terwarisi membuat Claude Code mematikan penyimpanan transcript.)
5. Kalau folder `cwd` sudah tidak ada: tampilkan pesan, dan jangan jalankan apa pun.

### 11.2 Session baru

1. User memilih folder.
2. App membuat UUID baru, lalu menjalankan `claude --session-id <uuid>`.
   Dengan begitu, app langsung tahu ID session tanpa harus menebak.
3. Session langsung muncul di grup Aktif.

### 11.3 Menutup dan menghentikan

- **Stop:** kirim Ctrl+C dua kali. Kalau proses belum berhenti dalam 3 detik, hentikan prosesnya. Session tetap ada di riwayat.
- **Menutup app:** semua session yang berjalan dicatat di `state.json` sebagai "terbuka sebelumnya",
  lalu prosesnya dihentikan dengan cara yang sama.
- **OS restart atau app crash:** `state.json` disimpan setiap ada perubahan, jadi daftar session terbuka tetap aman.

### 11.4 Setelah restart

- App membaca `state.json`, lalu menampilkan session tersebut di grup **Terbuka sebelumnya**.
- Session **tidak** dijalankan otomatis. Session baru jalan saat diklik (keputusan pemilik proyek).
- Opsional: app bisa diset untuk berjalan otomatis saat login.
- Tab "Terminal baru" yang hanya berisi shell (tanpa Claude) tidak dipulihkan.
  Tab yang sedang menjalankan Claude dipulihkan sebagai session di "Terbuka sebelumnya".

### 11.5 Terminal baru (deteksi otomatis)

1. User menekan **Terminal baru**. App membuka login shell user (`$SHELL -l`, atau `powershell.exe` di Windows)
   di folder tab yang sedang dipilih, atau di folder home.
2. Selama belum ada Claude, tab tampil di grup Aktif sebagai "Terminal · <folder>". Label folder mengikuti `cd`
   (dibaca dari `cwd` proses shell).
3. Deteksi Claude (cadangan, tanpa hook), dicek tiap ~1,5 detik:
   - Cari proses turunan dari shell yang terlihat seperti Claude Code: nama `claude`/`claude.exe`,
     `argv[0]` = `claude`, path `.../claude/versions/...`, atau argumen `@anthropic-ai/claude-code`.
   - Ambil `cwd` dan waktu mulai proses itu.
   - Session-nya = file `.jsonl` paling baru di `projects/<folder cwd>/` yang berubah setelah proses mulai,
     dan belum dipakai tab lain. Dicek ulang setiap kali, supaya `/clear` dan `/resume` ikut terdeteksi.
   - Kelemahan: dua tab yang memulai Claude di folder yang sama pada saat hampir bersamaan bisa tertukar.
     Hook (bagian 9) menghilangkan masalah ini.
4. Sebelum Claude menulis apa pun, tab tampil sebagai "claude · <folder>". Setelah ada `ai-title`, judul tab
   memakai judul itu (urutan judul bagian 8.3).
5. Saat `claude` keluar, tab kembali menjadi shell. Session tetap ada di riwayat. Menjalankan `claude` lagi
   di tab yang sama menghubungkan tab ke session baru.
6. Saat shell keluar (`exit`), tab ditutup.

### 11.6 Menghapus session

1. User memilih session, lalu menekan **Hapus**. App meminta konfirmasi: "Hapus permanen" atau "Batal".
2. Yang dihapus: `<projects>/<folder>/<id>.jsonl` dan folder `<projects>/<folder>/<id>/` (transcript subagent).
3. Pengaman:
   - Path harus tepat `<projects>/<folder>/<id>.jsonl` setelah di-resolve (tanpa `..` atau symlink keluar).
     Selain itu ditolak.
   - Session yang sedang berjalan di tab app tidak bisa dihapus. Tab harus ditutup dulu.
   - (Belum) Tolak juga kalau session terdeteksi berjalan di luar app (bagian 7).
4. Setelah dihapus, session hilang dari sidebar dan dari "Terbuka sebelumnya". Tidak bisa di-resume lagi.

## 12. Widget terminal (`crates/terminal`)

Diawali dari kode `iced_term` 0.8 (MIT). Atribusi dicantumkan di `NOTICE` dan di kepala file yang disalin.

### 12.1 Kebutuhan dari hasil tes Claude Code v2.1.296

Claude Code mengaktifkan mode berikut: `1000`, `1002`, `1003`, `1006`, `1049`, `1004`, `2004`, `2031`.
Klik dengan format SGR terbukti memindahkan kursor di kotak input.

| # | Kebutuhan | Kondisi di iced_term 0.8 | Yang harus dibuat |
|---|---|---|---|
| T1 | Klik kiri dilaporkan (SGR dan format lama) | ✅ Ada | Pertahankan + test |
| T2 | Drag dilaporkan (`1002`) | ✅ Ada | Pertahankan + test |
| T3 | **Scroll dilaporkan** sebagai tombol 64/65 saat mouse mode aktif | ❌ Diubah menjadi tombol ↑/↓ | **Wajib.** Laporkan scroll. Pakai ↑/↓ hanya kalau mouse mode mati dan alt screen + alternate scroll aktif |
| T4 | **Shift + drag** (Option + drag di macOS) memaksa seleksi terminal | ❌ Tidak ada | Tambahkan |
| T5 | Hover dilaporkan (`1003`) | ❌ Hanya saat drag | Laporkan gerakan tanpa tombol. Kirim hanya saat sel berubah |
| T6 | Klik kanan dan tengah dilaporkan | ❌ Tidak dipakai | Tambahkan |
| T7 | Focus events (`1004`): `ESC [ I` / `ESC [ O` | ❌ Tidak ada | Tambahkan |
| T8 | IME (preedit + commit) | ❌ Tidak ada | Hubungkan ke input method Iced 0.14. Tampilkan preedit di posisi kursor |
| T9 | Bracketed paste (`2004`) | ✅ Ada, dengan proteksi | Pertahankan + test |
| T10 | Seleksi: single / double (kata) / triple (baris) | ✅ Ada | Pertahankan |
| T11 | Copy / paste | ✅ Ada | ⌘C/⌘V di macOS, Ctrl+Shift+C/V di Windows/Linux |
| T12 | Notifikasi perubahan tema (`2031`) | ❌ | Kirim laporan saat mode terang/gelap OS berubah |
| T13 | Link (OSC 8 dan URL biasa) | ⚠️ Sebagian | ⌘/Ctrl + klik membuka link |
| T14 | Truecolor, bold/italic/underline, karakter lebar (emoji, CJK) | ✅ Dari alacritty_terminal | Test visual |
| T15 | Resize mengikuti ukuran area | ✅ | Kirim ukuran baru ke PTY (debounce) |

### 12.2 Banyak terminal sekaligus

- Setiap session yang berjalan punya satu `Terminal` (PTY + state emulator) yang tetap hidup walaupun tidak sedang ditampilkan.
- Hanya terminal yang sedang tampil yang digambar.
- Terminal di belakang tetap membaca output PTY, supaya buffer tidak penuh dan proses Claude tidak tertahan.

### 12.3 Target performa

- 60 fps saat Claude menulis output deras, di layar Retina dengan ukuran ~200×60 sel.
- Input keyboard muncul di layar dalam < 16 ms (satu frame).
- 10 terminal berjalan bersamaan: memori app < 300 MB (di luar proses `claude`).

## 13. Arsitektur

```
app/                         (repo)
├─ crates/
│  ├─ core/                  tanpa UI, tanpa Iced
│  │  ├─ sessions/           parser .jsonl, index, cache
│  │  ├─ watcher/            notify + debounce
│  │  ├─ status/             gabungan hook + PTY + proses luar
│  │  ├─ store/              state.json, sessions.json (atomic)
│  │  └─ launch/             membangun perintah + env per OS
│  ├─ terminal/              widget Iced + alacritty_terminal (bagian 12)
│  └─ app/                   UI Iced: sidebar, header, tab, pengaturan
├─ tests/e2e/                test ujung ke ujung (bagian 15)
├─ AGENTS.md                 panduan untuk AI agent (versi dikunci, aturan)
├─ NOTICE                    atribusi iced_term dan lainnya
└─ .github/workflows/        CI + rilis
```

- Pola Iced (Elm): `State` → `view` → `Message` → `update`.
- Pekerjaan berat (index, membaca file, PTY) berjalan di thread atau task terpisah. Hasilnya dikirim sebagai `Message`.
- `core` tidak bergantung pada `terminal` maupun `app`, supaya bisa dites sendiri dan bisa dipakai UI lain kelak.

## 14. Dukungan OS

| | macOS | Windows | Linux |
|---|---|---|---|
| PTY | Unix PTY | ConPTY (Windows 10 1809+) | Unix PTY |
| Folder session | `~/.claude` | `%USERPROFILE%\.claude` | `~/.claude` |
| Perintah | lewat `$SHELL -l -c` | `claude.exe` langsung | lewat `$SHELL -l -c` |
| Paket rilis | `.dmg` (signed + notarized) | `.msi` (signed kalau ada sertifikat) | AppImage + `.deb` |
| Jalan saat login | LaunchAgent | Registry Run key | `.desktop` di autostart |

**WSL (belum di v1):** session yang dibuat di dalam WSL tersimpan di filesystem Linux, bukan di Windows.
Dukungan ini butuh desain sendiri (membaca `\\wsl$\...` dan menjalankan lewat `wsl.exe`).

## 15. Testing

Mengikuti aturan proyek: setiap bug diawali dengan test (sebisa mungkin e2e) yang gagal, lalu diperbaiki.

| Level | Isi |
|---|---|
| Unit: `core` | Parser dengan fixture `.jsonl` (sudah dibersihkan dari data pribadi), urutan judul, pengelompokan waktu, simpan atomic |
| Unit: `terminal` | Untuk T1–T12: event masuk → **byte persis** yang dikirim ke PTY. Contoh: klik kiri di (8, 24) dengan SGR → `ESC[<0;9;25M` |
| E2E: terminal | Program uji kecil (`tests/e2e/fake-tui`) yang mengaktifkan mode yang sama dengan Claude Code, lalu mencatat semua input yang diterima. Test menjalankannya di PTY sungguhan, mengirim event lewat widget, lalu mengecek catatannya. Deterministik, tanpa jaringan |
| E2E: app | Jalankan app dengan folder `~/.claude` palsu berisi fixture, lalu cek sidebar, resume (dengan `claude` palsu), dan restore setelah restart |
| Manual (sebelum rilis) | Script tes mouse terhadap `claude` asli (mirip `mousetest.py`), di 3 OS |

CI menjalankan semua test di macOS, Windows, dan Linux.

## 16. Privasi dan keamanan

- Semua data tetap di komputer user. **Tidak ada jaringan, tidak ada telemetry.**
- File session hanya dibaca, tidak pernah diubah. Dihapus hanya atas perintah user, setelah konfirmasi.
- Hook hanya dipasang dengan persetujuan user, setelah perubahannya ditampilkan.
- Fixture test tidak boleh berisi data asli (kode, secret, email).
- README menjelaskan semua hal di atas dengan jelas.

## 17. Lisensi dan atribusi

- Usulan lisensi app: **MIT OR Apache-2.0** (umum di proyek Rust).
- `iced_term`: MIT → wajib mencantumkan copyright dan teks lisensinya di `NOTICE`.
- `alacritty_terminal`: Apache-2.0 → cocok.
- **Jangan** menyalin kode dari Zed (bagian terminalnya berlisensi GPL).

## 18. Tahapan

| Tahap | Isi | Selesai jika |
|---|---|---|
| **M0: Spike terminal** | Satu window Iced + satu terminal yang menjalankan `claude` | Claude Code bisa dipakai normal: klik memindahkan kursor, scroll, dan paste berjalan |
| **M1: Terminal lengkap** | T1–T15 + test byte + e2e `fake-tui` | Semua test lulus di 3 OS |
| **M2: Index + sidebar** | Parser, watcher, sidebar berkelompok, cari, resume | Semua session lokal tampil dalam < 1 detik (dengan cache) |
| **M3: Banyak tab + restore** | Banyak terminal, `state.json`, grup "Terbuka sebelumnya", **Terminal baru + deteksi Claude** (11.5) | Restart app → daftar session kembali; `claude` yang diketik di Terminal baru muncul di sidebar dengan judulnya |
| **M4: Status** | Status cadangan + hook opsional + deteksi proses luar | Indikator sesuai kondisi nyata |
| **M5: Rilis** | Paket 3 OS, signing, README, CONTRIBUTING | Rilis v0.1.0 di GitHub |

### 18.1 Status implementasi (2026-10-13): versi minimal untuk testing

Sudah ada:
- Workspace `crates/core`, `crates/terminal` (dari iced_term 0.8, sudah memakai alacritty_terminal 0.26), `crates/app`.
- Sidebar: Aktif, Terbuka sebelumnya, riwayat per waktu, kotak cari.
- Hapus session (11.6) dengan konfirmasi.
- Session baru (pilih folder → `claude --session-id`), Terminal baru + deteksi Claude, Lanjutkan (`claude --resume`).
- Status cadangan: ● bekerja (ada output < 2 detik), ◐ menunggu, ▸ shell.
- `state.json` disimpan secara atomic setiap ada perubahan.
- **T3 selesai**: scroll dilaporkan sebagai tombol 64/65 saat mouse mode aktif (SGR dan format lama),
  panah hanya saat mouse mode mati + alt screen + alternate scroll. Ada 3 test e2e dengan PTY sungguhan.
- Perbaikan CPU dari profiling: warna tema di-parse sekali (bukan per sel per frame); font diukur sekali
  (bukan per event PTY); event `Wakeup` beruntun digabung; tab tersembunyi tidak menyalin layarnya;
  sidebar riwayat memakai `lazy` (dibangun ulang hanya saat datanya berubah).
  Grid tidak lagi disalin: gambar langsung dari state terminal (dengan lock) hanya saat perlu digambar ulang.
  Biaya sync dengan scrollback penuh turun dari ~5,5 ms ke < 0,001 ms per event.
- Test unit `core`: parser, index bertahap, urutan judul, deteksi proses nyata, pencocokan session, simpan atomic.

Belum ada (sengaja ditunda):
- Watcher `notify` (sekarang masih polling tiap 1,5 detik, dan index-nya sudah membaca bertahap).
- Hook, deteksi session di luar app, status error merah, menu klik kanan, pin/arsip/ganti nama.
- Perbaikan terminal T4–T8, T12. Tombol Stop dengan Ctrl+C (sekarang "Tutup" menghentikan PTY).
- `index-cache.json`, e2e `fake-tui`, CI, paket rilis.

**Ide untuk v2:** catatan per session, ringkasan otomatis, notifikasi OS, dukungan WSL, split view (2 terminal berdampingan), statistik biaya dari `cost-state`.

## 19. Pertanyaan terbuka

1. Nama app dan nama repo?
2. Lisensi: repo saat ini memakai MIT (file `LICENSE`). Tetap MIT, atau MIT OR Apache-2.0?
3. Notifikasi OS saat session menunggu input: masuk v1 atau v2?
4. Menutup app: hentikan semua proses `claude`, atau tanya dulu kalau ada yang sedang bekerja?
5. Perlu fitur ekspor isi session (misalnya ke Markdown)?
