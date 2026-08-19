# 🛍️ TopUp Store Gateway (Server Web)

[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Framework: Axum](https://img.shields.io/badge/Framework-Axum-purple.svg)](https://github.com/tokio-rs/axum)

**TopUp Store Gateway** adalah server web *high-performance* berbasis Rust dan Axum untuk platform toko online topup game dan PPOB. Server ini bertindak sebagai **Server Web** yang mengelola antarmuka pengguna (CSR Frontend), katalog layanan game dinamis, pembuatan faktur pembayaran, autentikasi akun member, integrasi gateway pembayaran, serta verifikasi WhatsApp OTP.

---

## 🏛️ Arsitektur Sistem (Zero-Trust Microservices)

Sistem ini beroperasi dengan arsitektur **100% Dual Database Terpisah (*Shared-Nothing*)**:
- **Server Web (`TopUp_store_gateway`)**: Menangani antarmuka web publik, pengguna, penerbitan invoice, dan menerima Webhook (Tripay, Tokopay, Duitku, Paydisini) pada database lokalnya sendiri.
- **Server Topup (`TopUp_fulfillment_service`)**: Beroperasi di server terpisah dengan *Zero-Inbound Port*, mengeksekusi pesanan ke supplier DigiFlazz, dan membaca mutasi bot.
- **Komunikasi Antar-Server**: Dilakukan secara dua arah via **Telegram Bot Encrypted Queue (HMAC-SHA256)**.

---

## 📱 Integrasi WhatsApp Gateway (OpenWA / MPWA)

Server Web mendukung gateway WhatsApp untuk verifikasi pendaftaran (OTP), reset password, dan transaksi:

### Pilihan A: Menggunakan OpenWA (Rekomendasi: 100% Gratis & Self-Hosted)
Gunakan repository resmi [**rmyndharis/OpenWA**](https://github.com/rmyndharis/OpenWA):
1. Jalankan OpenWA di VPS menggunakan Docker:
   ```bash
   git clone https://github.com/rmyndharis/OpenWA.git
   cd OpenWA
   docker compose up -d
   ```
2. Buka dashboard OpenWA di browser (`http://YOUR_VPS_IP:3000`), lalu scan QR Code nomor WhatsApp Anda.
3. Hubungkan ke `.env` Server Web:
   ```env
   WA_GATEWAY_URL=http://127.0.0.1:3000/api/v1/send-message
   MPWA_API_KEY=api_key_dari_dashboard_openwa
   MPWA_SENDER_PHONE=default
   ```

### Pilihan B: Menggunakan MPWA Cloud
```env
WA_GATEWAY_URL=https://mpwa.byllann.com/send-message
MPWA_API_KEY=your_mpwa_api_key
MPWA_SENDER_PHONE=089667912348
```

---

## 🤖 Panduan Konfigurasi Telegram (Direct Admin Chat — Tanpa Grup)

Jalur komunikasi antar-server menggunakan **Direct Admin Chat** yang aman dan terenkripsi:

1. **Buat Bot di Telegram**:
   - Buka Telegram, cari **`@BotFather`** $\to$ ketik `/newbot`.
   - Masukkan nama bot dan username (contoh: `aruteru_topup_bot`).
   - Salin **Bot Token** yang diberikan (contoh: `7123456789:AAF_TokenBotAnda`).

2. **Dapatkan `TELEGRAM_ADMIN_CHAT_ID` (User ID Akun Anda)**:
   - Buka bot baru Anda di Telegram, lalu klik tombol **/start**.
   - Buka bot pembantu **`@userinfobot`** $\to$ klik `/start` untuk melihat ID akun Telegram Anda (contoh: `1234567890`).
   - Masukkan ID tersebut ke file `.env` sebagai `TELEGRAM_ADMIN_CHAT_ID`. Selesai! (Tidak perlu membuat grup).

---

## ⚙️ Prasyarat Sistem & Dependensi

- **Rust Toolchain**: `rustc` & `cargo` versi 1.75 atau lebih baru.
- **Database**: MySQL 8.0+ atau MariaDB 10.5+ (Lokal untuk Server Web).
- **Aset Web**: Folder `public/` (HTML, CSS, Vanilla JS, Gambar) yang sudah disertakan di repository.

---

## 🚀 Panduan Konfigurasi & Menjalankan Server

### 1. Konfigurasi Environment (`.env`)
Salin file `.env.example` menjadi `.env`:
```bash
cp .env.example .env
```

Sesuaikan parameter `.env`:
```env
# Server Web Configuration
SERVER_PORT=8080
SERVER_HOST=0.0.0.0

# Database Web Lokal (MySQL)
DATABASE_URL=mysql://db_user:db_password@localhost:3306/db_store_web

# Batas Hold Saldo Member
HOLD_BALANCE_MEMBER=0
HOLD_BALANCE_RESELLER=50000
HOLD_BALANCE_ADMIN=100000

# WhatsApp Gateway (OpenWA / MPWA)
WA_GATEWAY_URL=http://127.0.0.1:3000/api/v1/send-message
MPWA_API_KEY=your_wa_token
MPWA_SENDER_PHONE=default

# Payment Gateway Kredensial
TRIPAY_MERCHANT_CODE=T1234
TRIPAY_API_KEY=your_tripay_api_key
TRIPAY_PRIVATE_KEY=your_tripay_private_key

TOKOPAY_MERCHANT_ID=M1234
TOKOPAY_SECRET_KEY=your_tokopay_secret

DUITKU_MERCHANT_CODE=D1234
DUITKU_API_KEY=your_duitku_api_key

PAYDISINI_API_KEY=your_paydisini_key
PAYDISINI_MERCHANT_ID=your_merchant_id

# Komunikasi Terenkripsi ke Server Topup (Telegram)
TELEGRAM_BOT_TOKEN=7123456789:AAF_AbCdEfGhIjKlMnOpQrStUvWxYz12345
TELEGRAM_GROUP_2_ID=-1001234567890
TELEGRAM_ENCRYPTION_KEY=ARUTERU_SECRET_KEY_SUPER_SECURE_2026
```

### 2. Menjalankan Server
```bash
# Mode Development
cargo run --bin store_gateway

# Mode Produksi (Optimasi Penuh)
cargo run --bin store_gateway --release
```

Server Web akan aktif di `http://0.0.0.0:8080` dan melayani seluruh request website beserta API-nya.

---

## 📄 Lisensi
Proyek ini dilindungi di bawah lisensi **GNU AGPLv3 (Affero General Public License v3.0)**.
