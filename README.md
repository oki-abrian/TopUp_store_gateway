# 🛍️ TopUp Store Gateway (Server Web)

[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://gnu.org/licenses/agpl-3.0)
[![Framework: Axum](https://img.shields.io/badge/Framework-Axum-purple.svg)](https://github.com/tokio-rs/axum)

**TopUp Store Gateway** adalah server web *high-performance* berbasis Rust dan Axum untuk platform toko online topup game dan PPOB. Server ini mengelola antarmuka pengguna (CSR Frontend), katalog layanan game dinamis, autentikasi akun member, penerbitan faktur pembayaran, verifikasi WhatsApp OTP, serta API reseller.

> ⚠️ **Repo ini HANYA Server Web.** Eksekusi topup dilakukan oleh proyek terpisah — **Fulfillment Service / Server Topup** (`fulfillment_backend`). Kedua proyek di-deploy di server yang berbeda.

---

## 🏛️ Arsitektur Sistem (Zero-Trust Dual Database)

Sistem beroperasi dengan **100% dual database terpisah (*shared-nothing*)**:

| Komponen | Peran | Port |
|---|---|---|
| **Server Web** (repo ini) | Website publik, user, invoice gateway, webhook pembayaran | HTTP terbuka |
| **Server Topup** (`fulfillment_backend`) | Validasi harga & voucher otoritatif, eksekusi topup supplier | **Zero-inbound** (tanpa port) |
| **Komunikasi** | Antrean Telegram terenkripsi dua arah (HMAC-SHA256 + anti-replay 120 detik) | — |

### 🔒 Two-Phase Checkout (Web Tidak Dipercaya)

Harga **TIDAK boleh dipercaya dari sisi web** — alur wajib:

```
submit order ──[NEW_ORDER]──▶ SERVER TOPUP hitung harga ASLI dari DB Topup
                                    │
            [ORDER_ACCEPTED] ◀──────┘  (harga resmi dikunci di web)
                    │
      faktur otomatis membuat tagihan gateway SETELAH ACC
                    │
            user bayar harga terverifikasi → webhook → topup
```

Order ditolak (`[ORDER_REJECTED]`)? Invoice **tidak pernah dibuat** — nol uang masuk, nol refund. Web diretas sekalipun tidak bisa underprice atau memalsukan kupon.

### 🎟️ Pengamanan Voucher 3 Level
1. **L1** — dedup per-batch Telegram: dari N pesanan identik (user+voucher), hanya yang pertama diproses.
2. **L2** — validasi penuh di Server Topup: stok atomik, kategori, minimum belanja, eksklusif flashsale.
3. **L3** — anti-replay: voucher yang masih tertaut transaksi aktif/sukses lain → diskon dicabut otomatis.

---

## 📱 Integrasi WhatsApp Gateway (OpenWA / MPWA)

### Pilihan A: OpenWA (Rekomendasi: Gratis & Self-Hosted)
Gunakan [**rmyndharis/OpenWA**](https://github.com/rmyndharis/OpenWA):
```bash
git clone https://github.com/rmyndharis/OpenWA.git && cd OpenWA
docker compose up -d   # lalu scan QR Code di http://VPS_IP:3000
```

### Pilihan B: MPWA Cloud
Langsung isi `WA_GATEWAY_URL=https://mpwa.byllann.com/send-message`.

---

## 🤖 Menyiapkan Bot Telegram & Group ID

Komunikasi antar-server memakai beberapa bot sekaligus (pembagian kuota ±20 call/menit per bot):

| Variabel | Fungsi |
|---|---|
| `TELEGRAM_BOT_SENDER_TOKEN` | Pengirim antrean batch utama (dari web ke topup) |
| `TELEGRAM_BOT_4_TOKEN` | Pengirim kedua — round-robin, menggandakan kuota (siklus 5 detik aman) |
| `TELEGRAM_CALLBACK_BOT_TOKEN` | Listener status di sisi web (**wajib beda bot dengan milik fulfillment**) |

Cara membuat: **@BotFather → `/newbot`**, jadikan bot admin grup, ambil Chat ID via **@raw_data_bot** (format `-100xxxxxxxxxx`). Semua bot diletakkan di grup yang sama, tanpa anggota manusia.

> 🚨 `TELEGRAM_ENCRYPTION_KEY` **wajib sama** dengan milik Server Topup. Dua listener tidak boleh memakai token bot yang sama (tabrakan getUpdates/HTTP 409).

---

## ⚙️ Prasyarat

- Rust toolchain 1.85+
- MySQL 8.0+ / MariaDB 10.5+ (database lokal web)
- Folder `public/` (HTML/CSS/JS vanilla — sudah termasuk)

## 🚀 Konfigurasi & Menjalankan

```bash
cp .env.example .env   # lalu sesuaikan
```

```env
SERVER_PORT=8080
SERVER_HOST=0.0.0.0
DATABASE_URL=mysql://user:pass@localhost:3306/db_store_web

# Payment Gateway (isi sesuai yang dipakai)
TRIPAY_MERCHANT_CODE=...      TRIPAY_API_KEY=...       TRIPAY_PRIVATE_KEY=...
TOKOPAY_MERCHANT_ID=...       TOKOPAY_SECRET_KEY=...
DUITKU_MERCHANT_CODE=...      DUITKU_APIKEY=...
PAYDISINI_API_KEY=...         PAYDISINI_MERCHANT_ID=...

# URL publik untuk callback webhook gateway
PUBLIC_BASE_URL=https://domainanda.com

# WhatsApp OTP
WA_GATEWAY_URL=...
MPWA_API_KEY=...
MPWA_SENDER_PHONE=...

# Telegram bus komunikasi
TELEGRAM_BOT_SENDER_TOKEN=...
TELEGRAM_BOT_4_TOKEN=...
TELEGRAM_CALLBACK_BOT_TOKEN=...
TELEGRAM_GROUP_2_ID=-100xxxxxxxxxx
TELEGRAM_ENCRYPTION_KEY=kunci-rahasia-sama-dengan-server-topup
```

```bash
cargo run --bin store_gateway --release
```

Server aktif di `http://0.0.0.0:8080`.

---

## 🔌 API Reseller

`POST /api/v1/external/order` (auth: `ukey` + signature MD5 + IP whitelist) bersifat **dua fase**: respons langsung berstatus `pending`, topup dieksekusi Server Topup setelah validasi saldo & harga. Pantau hasilnya lewat endpoint status hingga `success`/`error`.

## 📄 Lisensi

Proyek ini dilindungi di bawah lisensi **GNU AGPLv3**.
