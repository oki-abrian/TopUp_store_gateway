# FEATURE AUDIT REPORT — Aruteru Shoppu (PHP vs Rust Backend)

> **Tujuan:** Dokumen ini berisi DUA laporan inventaris fitur yang dihasilkan dari eksplorasi codebase:
> 1. **Laporan A** — Inventaris fitur Rust backend (`rust_backend/`)
> 2. **Laporan B** — Inventaris fitur aplikasi PHP asli (root: `index.php`, `admin/`, `api/`, `auth/`, `account/`, `page/`, `library/`)
>
> Dibuat: 21 Agustus 2026. Kedua laporan disimpan apa adanya (verbatim) agar dapat dibandingkan oleh AI/reviewer lain.
> Catatan konteks: skema pembayaran versi Rust MEMANG BERBEDA by design (Telegram-queue fulfillment, webhook per gateway). Fokus audit = fitur & tampilan web.

---

# LAPORAN A — FEATURE INVENTORY: Rust Backend

Root: `/Users/macmini/.mounty/SSD_External/Aruteru Shoppu NEW/rust_backend`

## 0. Architecture Overview

Three binaries declared in `Cargo.toml` (Axum 0.7 + SQLx MySQL + teloxide):

| Binary | Path | Role |
|---|---|---|
| `rust_backend` | `src/main.rs` | Legacy/minimal monolith entrypoint (5 routes). Not the primary server. |
| `store_gateway` | `src/bin/store_gateway.rs` | **The real web server** (~2,671 lines): all pages, APIs, webhooks, static file serving. |
| `fulfillment_service` | `src/bin/fulfillment_service.rs` | **Zero-inbound worker** (opens no HTTP port): cron workers + 2 Telegram bots that talk to DigiFlazz. |

Inter-service communication is done via an encrypted Telegram bot queue (HMAC-SHA256 signed, base64 payloads, `ENC:` prefix, anti-replay 120s window) instead of HTTP.

## 1. ROUTES

### 1a. `src/main.rs` (binary `rust_backend`) — via `src/routes.rs::create_router`

| Method | Path | Handler |
|---|---|---|
| GET | `/health` | `health_check` |
| POST | `/api/order` | `handle_api_order` → `orders/engine::process_api_order` |
| GET | `/api/service` | `handle_list_services` |
| GET | `/api/check-ml` | `handle_check_ml` → SmileOneChecker |
| GET | `/api/check-game` | `handle_check_game` → DuniaGamesChecker |

### 1b. `src/bin/store_gateway.rs` (the live site)

**Health / diagnostics**
| Method | Path | Handler |
|---|---|---|
| GET | `/health` | `health_check` |

**Static HTML pages (pretty URLs mirroring old PHP routes)** — all served by `serve_static_page` reading `public/<name>.html`
| Method | Path | Handler (serves file) |
|---|---|---|
| GET | `/invoices` | `serve_invoices_page` → `invoice.html` |
| GET | `/invoices/:oid` | `serve_faktur_page` → `faktur.html` |
| GET | `/product/:type/category/:slug` | `serve_order_page` → `order.html` |
| GET | `/blog/:slug` | `serve_blog_page` → `blog.html` |
| GET | `/auth/login` | `serve_login_page` → `login.html` |
| GET | `/auth/register` | `serve_register_page` → `register.html` |
| GET | `/auth/forgot-password` | `serve_forgot_page` → `forgot.html` |
| GET | `/account`, `/account/` | `serve_account_page` → `account.html` |
| GET | `/admin`, `/admin/` | `serve_admin_page` → `admin.html` |
| GET | `/page/region` | `serve_region_page` → `region.html` |
| GET | `/page/pricelist` | `serve_pricelist_page` → `pricelist.html` |
| GET | `/page/privacy-policy` | `serve_privacy_page` → `privacy.html` |
| GET | `/page/terms-and-condition` | `serve_terms_page` → `terms.html` |
| GET | `/page/review` | `serve_review_page` → `review.html` |
| GET | `/leaderboard` | `serve_leaderboard_page` → `leaderboard.html` |
| (fallback) | `/` and everything else | `ServeDir::new("public")` → serves `index.html` at root |
| (nest) | `/library/*` | `ServeDir::new("public/library")` |
| (nest) | `/imagehome/*` | `ServeDir::new("public/imagehome")` |

**Public catalog / game-check API**
| Method | Path | Handler |
|---|---|---|
| GET | `/api/service` | `handle_list_services` |
| GET | `/api/check-ml` | `handle_check_ml` (SmileOne MLBB) |
| GET | `/api/check-game` | `handle_check_game` (DuniaGames multi-game) |
| GET | `/api/check-pubg` | `handle_check_pubg` (PUBG checker) |
| GET | `/api/v1/page-data` | `handle_page_data` (banners, tabs, categories, news, flashsale, site config) |
| GET | `/api/v1/search?q=` | `handle_search` |
| GET | `/api/v1/product/:type/:slug` | `handle_product` |
| GET | `/api/v1/invoices` | `handle_invoices` (last 10 transactions) |
| GET | `/api/v1/invoice/:oid` | `handle_invoice_detail` |
| GET | `/api/v1/blog/:slug` | `handle_blog` |
| GET | `/api/v1/leaderboard` | `handle_leaderboard` (daily/weekly/monthly) |
| GET | `/api/v1/pricelist/:code` | `handle_pricelist` |
| GET | `/api/v1/payment-methods` | `handle_payment_methods` (from `metode` table) |
| GET | `/api/v1/reviews` | `handle_reviews` (**always returns empty array**) |
| POST | `/api/v1/order/submit` | `handle_submit_order` |
| POST | `/api/v1/review/submit` | `handle_review_submit` (best-effort insert into `ulasan`; silently succeeds without persisting if table absent) |
| POST | `/api/v1/forgot-password` | `handle_forgot_password` (reset + WA new password) |
| POST | `/api/invoice/create` | `handle_create_invoice` (Tripay/Duitku/Tokopay/Paydisini/SALDO) |
| POST | `/api/invoice/verify-otp` | `handle_verify_otp` (relays `[VERIFY]` to Telegram) |

**Auth**
| Method | Path | Handler |
|---|---|---|
| POST | `/api/auth/login` | `handle_login` (+ Turnstile verify) |
| POST | `/api/auth/register` | `handle_register` (creates pending session, sends WA OTP) |
| POST | `/api/auth/register-step2` | `handle_register_step2` (verify OTP, create user, notify admin via WA) |
| POST | `/api/auth/register-resend` | `handle_register_resend` (120s cooldown) |
| GET | `/api/config/turnstile` | `handle_config_turnstile` (sitekey from `config` table) |

**Account (member dashboard)** — *note: authenticated only client-side; username passed as query/body param*
| Method | Path | Handler |
|---|---|---|
| GET | `/api/account/profile?username=` | `handle_profile` |
| GET | `/api/account/dashboard?username=` | `handle_account_dashboard` (balance, counts, monthly revenue) |
| GET | `/api/account/mutations?username=` | `handle_mutations` |
| GET | `/api/account/transactions?username=` | `handle_transactions` |
| POST | `/api/account/generate-keys` | `handle_generate_keys` |
| POST | `/api/account/update-profile` | `handle_update_profile` |
| POST | `/api/account/change-password` | `handle_change_password` (bcrypt verify) |
| GET | `/api/account/api-settings?username=` | `handle_api_settings` |
| POST | `/api/account/update-api` | `handle_update_api_settings` |
| GET | `/api/account/activity?username=` | `handle_activity` (users_cookie sessions, fallback mutation log) |
| GET | `/api/account/deposit/methods` | `handle_deposit_methods` (**hardcoded mock list**: QRIS/OVO/GOPAY/DANA/LINKAJA/SHOPEEPAY) |
| GET | `/api/account/deposit/history?username=` | `handle_deposit_history` |
| POST | `/api/account/deposit/create` | `handle_deposit_create` → `account::service::create_deposit` |

**Admin** — *no server-side auth check on any of these*
| Method | Path | Handler |
|---|---|---|
| POST | `/api/admin/service/create` | `handle_admin_create_service` |
| POST | `/api/admin/user/balance` | `handle_admin_adjust_balance` |
| POST | `/api/admin/user/lock` | `handle_admin_lock_account` |
| GET | `/api/admin/financial-summary` | `handle_admin_financial_summary` |
| GET | `/api/admin/services` | `handle_admin_list_services` |
| GET | `/api/admin/transactions` | `handle_admin_list_transactions` |
| GET | `/api/admin/users` | `handle_admin_list_users` |
| GET/POST | `/api/admin/config/website` | `handle_admin_config_website_get` / `_post` |
| POST | `/api/admin/transaction/update-status` | `handle_admin_update_transaction_status` |

**External reseller API (key+sign MD5 auth, mirrors old `api/*.php`)**
| Method | Path | Handler |
|---|---|---|
| POST | `/api/v1/external/service` | `handle_external_service` (list services w/ filters) |
| POST | `/api/v1/external/status` | `handle_external_status` (order status lookup) |

**Payment webhooks/callbacks**
| Method | Path | Handler | Signature scheme |
|---|---|---|---|
| POST | `/webhook/tripay` | `handle_tripay_webhook` | HMAC-SHA256 of raw body vs `x-callback-signature` header; secret = `provider.userid` where code='TRIPAY' |
| POST | `/webhook/duitku` | `handle_duitku_webhook` | MD5(merchantCode+amount+merchantOrderId+apiKey); success = resultCode "00" |
| POST | `/webhook/tokopay` | `handle_tokopay_webhook` | MD5(merchant:secret:reff_id); success = status "success" |
| POST | `/webhook/paydisini` | `handle_paydisini_webhook` | MD5(apikey+uniqueCode+"CallbackStatus"); success = status "success" |

All four: validate ID safety (`is_safe_id`), branch on `DP*` prefix → mark deposit paid + credit user balance (transactional); otherwise mark transaction `payment_status='paid'` and notify fulfillment via Telegram.

### 1c. `src/bin/fulfillment_service.rs`
No HTTP routes at all. Starts: `status_poller`, `expired_cleaner`, `tokopay_worker`, `gopay_bot` (Bot 1), `report_bot` (Bot 2).

## 2. HANDLERS / SERVICES BY DOMAIN MODULE

### `src/domain/account/service.rs`
- Structs: `MutationLog`, `UpdateProfileRequest`, `ChangePasswordRequest`, `UpdateApiSettingsRequest`, `DepositRequest`, `ActivityLog`, `DepositLog`.
- `get_user_profile` – fetch user row by username.
- `get_user_mutations` – last 100 balance mutations.
- `get_user_transactions` – last 100 transactions.
- `generate_api_keys` – delete + regenerate UID/UKEY pair in `users_api`.
- `update_api_settings` – update whitelist + callback URL.
- `create_deposit` – validates amount against `metode` min/max, computes fee/unique code, generates `DPyyyymmddhhmmNN` id, creates invoice via **Paydisini or Tripay** gateways (real API calls); Tokopay/Duitku get **hardcoded fake checkout URLs** (`https://tokopay.co.id/checkout/{id}`, `https://duitku.com/checkout/{id}`); provider "X" uses method guide text; inserts `deposit` row; notifies fulfillment `[NEW_DEPOSIT]`. Return URL is placeholder `https://yoursite.com/deposit/invoices/{id}`.

### `src/domain/admin/service.rs`
- `AdminCreateServiceRequest`, `AdminUpdateUserBalanceRequest`, `AdminLockAccountRequest`, `FinancialSummary`.
- `admin_create_service` – INSERT into `service` (status 'available').
- `admin_adjust_user_balance` – FOR UPDATE lock, add/deduct balance + mutation log row.
- `admin_lock_user_account` – delete+insert into `users_lock` (note: login never checks this table).
- `admin_get_financial_summary` – totals: transactions, sales volume, gross profit, pending/success counts.

### `src/domain/admin/config.rs`
- `AdminConfigWebsiteRequest` (title/navbar/description/keyword/banner/icon).
- `get_website_config` – reads `config WHERE name='webcfg'` params 1–6.
- `update_website_config` – six UPDATEs, one per parameter.

### `src/domain/admin/transaction.rs`
- `AdminUpdateTransactionStatusRequest` (invoice, status).
- `update_transaction_status` – `UPDATE transaction SET status=? WHERE web_invoice=?` (**column name `web_invoice` likely mismatches schema's `order_id`**). WhatsApp notification noted as TODO comment.

### `src/domain/auth/service.rs` (+ `models.rs`)
- `normalize_phone` – digits only, normalize to 62-prefix.
- `verify_turnstile` – Cloudflare Turnstile siteverify call.
- `send_whatsapp_message` – universal WA gateway sender (OpenWA JSON Bearer style OR MPWA form style, chosen by URL heuristics).
- `send_mpwa_otp` – thin wrapper over send_whatsapp_message.
- `login_user` – bcrypt verify, insert session row into `users_cookie` (7-day expiry, SESS_ token). **Does not check `users_lock`.**
- `register_user` – duplicate username/email/phone checks, bcrypt hash, INSERT user level 'Member'.
- Models: `PendingRegistration`, `LoginRequest`, `RegisterRequest`, `VerifyOtpRequest`, `ResendOtpRequest`, `AuthResponse`.

### `src/domain/orders/engine.rs`
- `verify_signature` – MD5(uid+ukey) == sign.
- `check_ip_whitelist` – "*" or comma list.
- `process_api_order` – full external-API order pipeline: auth key/sign/IP → lock user row → price by level (Reseller/Admin=reseller profit else member) → flashsale discount → hold-balance enforcement → deduct balance + mutation + transaction ('process'/'paid') → if provider DIGI, call DigiFlazz topup; auto-refund + mark error on failure.

### `src/checkers/*` (duplicate copies also under `src/domain/checkers/pubg.rs`)
- `smileone_mlbb.rs::SmileOneChecker` – `check_role(user_id, zone_id)` posts to smile.one MLBB checkrole; maps region codes (ID/MY/SG/...) to names.
- `duniagames.rs::DuniaGamesChecker` – `check(user_id, zone_id, game_code)` GETs `https://cek.rizkydev.web.id/api/game/{code}`.
- `pubg.rs::PubgChecker` – `check(char_id)` GETs `https://cek-id-game.vercel.app/api/game/pubg-mobile-global-vc`.

### `src/domain/cron/*`
- `status_poller.rs::start_status_poller_task` – every 30s polls DigiFlazz `check_status` for `provider='DIGI' AND status='process'` orders; marks success (with SN) or error; refunds balance only for Saldo-paid orders; broadcasts `[STATUS_UPDATE]`/`[REFUND_USER]`.
- `auto_refund.rs::start_expired_cleaner_task` – every 60s expires unpaid transactions (>15 min) and cancels stale deposits.
- `tokopay_worker.rs::start_tokopay_worker` – every 60s polls Tokopay API for unpaid Tokopay **deposits** (credit balance atomically) and unpaid Tokopay **transactions** (calls `process_paid_transaction` → DigiFlazz).

### `src/domain/telegram/*`
- `mod.rs::process_paid_transaction` – core fulfillment: fetch order, reject underpayment, atomic claim (`payment_status='paid' AND status='processing'` guard against double-processing), dispatch DigiFlazz topup; instant-success/pending/failed branches; auto-refund saldo buyers; broadcast status back to web DB via Telegram.
- `sender.rs` – global unbounded MPSC queue + batch worker (6s cycles, ≤50 msgs, 3000-char chunks, retry/backoff honoring Telegram 429 `retry_after`, re-queue on failure); `encrypt_telegram_payload` (HMAC-SHA256 + base64 + timestamp/nonce); `send_report_to_fulfillment` enqueues `[REPORT]`/command messages; `init_telegram_sender`.
- `report_bot.rs` – Bot 2 listener: decrypts envelopes (signature + anti-replay), deletes non-text/plain messages, then executes commands: `[NEW_DEPOSIT]` (upsert user + deposit), `[REQ_OTP]` (create pending trx, generate OTP stored in `note` as `OTP:CODE:TS:ATTEMPTS`, send via WA), `[NEW_ORDER]` (upsert transaction), `[VERIFY]` (validate OTP w/ 10-min expiry + 3 attempts, deduct saldo, run topup), `[REPORT]/ORD...` (verify + topup with race-condition retries).
- `gopay_bot.rs` – Bot 1 listener: parses "Rp<amount>" from GoPay notification texts, FIFO-matches an unpaid QRIS/GOPAY order by exact price, triggers `process_paid_transaction`.

### `src/domain/whatsapp/mpwa.rs`
- `MpwaClient` – MPWA JSON sender (endpoint hardcoded `https://mpwa.byllann.com/send-message`).
- `format_order_success_message` – builds Indonesian success template.
- `send_whatsapp_notification` – posts payload; returns bool. *(Module appears largely unused — auth/service.rs has its own sender.)*

### `src/providers/digiflazz.rs`
- `DigiFlazzClient::generate_signature` (MD5 user+key+ref), `topup` (POST /v1/transaction; classifies sukses/gagal/pending; masks "saldo" errors), `generate_status_signature`, `check_status` (commands=status).

## 3. MODELS (`src/models.rs`)

| Struct | Fields |
|---|---|
| `User` | id, username, password?, name?, email?, phone?, balance, level (Member/Reseller/Admin), sso?, date_cr?, date_up? |
| `UserApi` | id, user, uid, ukey, whitelist, callback?, date_cr? |
| `Service` | id, code, name, game, type_name(`type`), price, member, reseller, provider, status (available/empty) |
| `Category` | id, name, code, type_name, prefix, data_form?, status |
| `Transaction` | id, order_id, order_tid?, provider_order_id?, user, code, service_name, game, target, price, profit, status (pending/process/success/error), payment_status (unpaid/paid), provider, note?, flashsale?, created_at?, updated_at?, expired_at?, callback? |
| `Provider` | id, code, userid, apikey, merchant?, link, mode? |
| `Flashsale` | id, code, amount, status ("1" active) |
| `ApiOrderRequest` | key, sign, service, target |
| `ApiOrderDataResponse` | order_id, data, code, service, status, note, price |
| `ApiOrderResponse` | result, data?, message |

**No model structs exist for**: Deposit, Voucher, Banner, Blog/News, Settings/Config, Metode (payment methods), Mutation — these are all queried ad-hoc via raw SQL + `serde_json::json!` in handlers. Domain-local models: `MutationLog`, `ActivityLog`, `DepositLog` (account), `PendingRegistration` etc. (auth), plus serialization DTOs in store_gateway (`BannerData`, `TabData`, `CategoryData`, `NewsData`, `FlashsaleData`, `SiteConfigData`, `PopupData`).

Other infra: `AppState` (db pool, http client, config, `pending_registers: DashMap`), `Config`, `AppError` enum (14 variants incl. `HoldBalanceViolation`, `RateLimitTriggered`, `AccountLocked` — the latter two effectively unused).

## 4. STATIC WEB PAGES (`public/`)

Shared stack: Alpine.js 3 + Tailwind CDN + jQuery + Bootstrap 5; shared JS `public/assets/js/app-data.js` (fetchJSON, fmt rupiah, copyToClipboard, hideString masking, showToast, applyConfigColors, `layoutData()` calling `/api/v1/page-data` + `/api/v1/search`) and `public/assets/js/layout.js` (injects navbar/search modal/popup/footer/CS chat into `#site-header`/`#site-footer` when `<body data-aruteru-layout>`; theme toggle; login-state branching Masuk↔Dashboard via localStorage `auth_token`/`auth_user`).

| File | Purpose / UI sections | API endpoints called | Status |
|---|---|---|---|
| `index.html` | Home: hero Swiper banner carousel, flashsale marquee w/ countdown, Trending (popular games), tabbed category grid (12-per-tab + load more, two UI modes), Berita Terkini news cards, bubbles/meteors effects | `/api/v1/page-data` | Complete. Served at `/` via fallback ServeDir. |
| `login.html` | Login card: username/password, show/hide, remember, Turnstile widget, link to register/forgot | `/api/config/turnstile`, `/api/auth/login` | Complete. Stores token+user in localStorage. |
| `register.html` | 2-step register: step 1 form (name/username/email/WA/password+confirm, Turnstile), step 2 OTP w/ resend cooldown | `/api/config/turnstile`, `/api/auth/register`, `/api/auth/register-step2`, `/api/auth/register-resend` | Complete. |
| `forgot.html` | Reset password via registered WA number | `/api/v1/forgot-password` | Complete. |
| `account.html` | Member dashboard SPA: auth gate; sidebar (Dashboard, Hall of Fame, Riwayat, Deposit New/History, API docs links, Pages links, Profil, Mutasi, Aktivitas, Admin Panel link if Admin, Keluar); content tabs: dashboard stats cards, mutations table, history table, activity table, deposit form + history, profile detail/edit/password/API-keys tabs | `/api/account/dashboard`, `/mutations`, `/transactions`, `/profile`, `/api-settings`, `/update-profile`, `/change-password`, `/generate-keys`, `/update-api`, `/activity`, `/deposit/methods`, `/deposit/history`, `/deposit/create` (+ layout `/api/v1/page-data`) | Mostly complete. **Stub:** "Hall of Fame" tab is empty placeholder ("This could call the existing leaderboard API"). **Bug:** after deposit create redirects to `/invoices/` + `res.data.invoice_id`, but backend returns `deposit_id` (top-level), so redirect goes to `/invoices/undefined`. |
| `admin.html` | Single-page admin SPA: access gate (level==='Admin'); big sidebar (Dashboard Devices/Statistics/Revenue; Users Manage/Locked/Mutation/Activity; Configuration Website/Terms/FAQ/Prefix/Others; Others Banner/Social/Color/Notification/PopUp/Blog; Tabs/Category/Service; Provider; Transaction Manage/Manual-Notify/Report/Voucher/Flashsale; Financial Sales/Profit/Summary; Deposit Manage/Report/Method; Server Act Cronsjob/Error Log; Get Layanan DIGIFLAZZ/VIPAYMENT/KEYPEDIA; Keluar). Implemented panes: financial summary cards, services table, transactions table, users table | `/api/admin/financial-summary`, `/api/admin/services`, `/api/admin/transactions`, `/api/admin/users` (+ layout page-data) | **Heavily stubbed.** Only 3–4 panes work; ~20 sidebar items are dead `href="#"` links. Backend endpoints that exist but are never wired into UI: `/api/admin/service/create`, `/api/admin/user/balance`, `/api/admin/user/lock`, `/api/admin/config/website` (GET/POST), `/api/admin/transaction/update-status`. Services pane even admits "+ Tambah via form admin (belum tersedia di CSR)". |
| `order.html` | Product/order page: category banner+thumbnail, trust badges, desktop testimonials block (avg rating, star bars, review list), Step 1 data-account dynamic fields, Step 2 nominal grid w/ variants + skeleton loader + discount ribbons, Step 3 payment methods (fee calc, BEST PRICE badge), Step 4 voucher code, Step 5 WhatsApp number, confirm modal, submit | `/api/v1/product/{type}/{slug}`, `/api/v1/payment-methods`, `/api/v1/reviews`, `/api/v1/voucher/check` (**endpoint does not exist in backend**), `/api/v1/order/submit` (+ page-data) | Functional except voucher check will always fail (404→"Kode voucher tidak valid"). Note: backend `data_form` is always empty so Step 1 renders zero inputs currently. |
| `faktur.html` | Invoice detail (`/invoices/:oid`): payment hero, product image/category/service, ID/nickname/region w/ copy buttons, payment method, invoice number/status chips, rincian pembayaran breakdown, total w/ copy, progress timeline (created→paid→success/error/system), print button, payment instructions (QRIS image / VA number+AN copy / e-wallet pay link), yellow QRIS hint box, review form (stars + message) when success+paid | `/api/v1/invoice/{oid}` (load + 2s polling reload), `/api/v1/review/submit` | Complete. |
| `invoice.html` | Invoice search page (`/invoices`): search box → redirect to faktur; real-time latest-transactions table | `/api/v1/invoices`, `/api/v1/invoice/{oid}` | Complete. |
| `blog.html` | Blog article viewer: banner, title, base64-decoded HTML content | `/api/v1/blog/{slug}` | Complete (minimal). |
| `leaderboard.html` | Top spenders: three boxes (Top 3 daily, Top 5 weekly, Top 10 monthly) with rank medals | `/api/v1/leaderboard` | Complete. |
| `pricelist.html` | Price list: category dropdown (built from page-data categories) + refresh button; table Kode/Nama/Harga Tamu/Member/Reseller/Status | `/api/v1/page-data`, `/api/v1/pricelist/{code}` | Complete. |
| `region.html` | MLBB region checker: user id + server form, result table (username/region) | `/api/check-ml` | Complete. |
| `review.html` | Testimonials wall: card grid (category, stars, message, masked order id, name/date) + load-more | `/api/v1/reviews` | UI complete but **always empty** because backend returns `[]`. |
| `privacy.html` | Privacy policy page; content decoded from `config.pages[0]` (base64) | `/api/v1/page-data` (via loadConfig) | Content depends on DB conf; falls back to "Konten belum tersedia." |
| `terms.html` | Terms & conditions; content from `config.pages[1]` | `/api/v1/page-data` | Same caveat. |

Assets: `public/library/assets/guest/css/*.css` (app/header/footer/home/order/etc.), `public/imagehome/` (logo, footer art, background, banners, ~dozens of game SVG icons), `public/assets/js/{app-data,layout}.js`.

## 5. PAYMENTS (`src/payments/`)

| Gateway | File | Create-transaction | Webhook | Notes |
|---|---|---|---|---|
| **Tripay** | `tripay.rs` | Yes — HMAC-SHA256 request signature, order items, sandbox/prod base URLs, returns checkout_url/qr_string/pay_code | `/webhook/tripay` in store_gateway (re-implements signature verification inline rather than calling `verify_webhook_signature`) | Used for both order invoices and deposits. |
| **Duitku** | `duitku.rs` | Yes — MD5 signature, v2 inquiry endpoint, callbackUrl wired to `http://127.0.0.1:8081/webhook/duitku` (points at fulfillment port!) | `/webhook/duitku` | Deposits via Duitku only build a fake `https://duitku.com/checkout/{id}` URL (not a real API call). |
| **Tokopay** | `tokopay.rs` | Yes — MD5(merchant:secret:reff), items array | `/webhook/tokopay` + background `tokopay_worker` polling reconciliation | Deposits via Tokopay also just build a fake `https://tokopay.co.id/checkout/{id}` URL. |
| **Paydisini** | `paydisini.rs` | Yes — MD5(key+unique+service+amount+validTime+"NewTransaction"), form-encoded, 30-min validity | `/webhook/paydisini` | Fully used for deposits too. |

Additional payment paths: **SALDO** (account balance) via `[REQ_OTP]`/`[VERIFY]` Telegram OTP relay; **GoPay/QRIS manual matching** via gopay_bot reading bot-forwarded payment notifications; provider "X" manual transfer instructions. Credentials come from either env vars (invoice creation) or the `provider` DB table (webhooks/deposits).

## 6. CONFIG (`src/config.rs` + `.env.example`)

Struct fields: `server_port`, `server_host`, `database_url`, `redis_url`, `hold_member/reseller/admin`, `trx_interval_seconds`, `mpwa_api_key`, `mpwa_sender_phone`; helper `get_hold_balance(level)`.

Env vars in `.env.example`: `SERVER_PORT`, `SERVER_HOST`, `DATABASE_URL`, `REDIS_URL`, `HOLD_BALANCE_MEMBER/RESELLER/ADMIN`, `TRX_INTERVAL_SECONDS`, `WA_GATEWAY_URL`, `MPWA_API_KEY`, `MPWA_SENDER_PHONE`, `MPWA_ADMIN_PHONE`, `DIGIFLAZZ_USERNAME`, `DIGIFLAZZ_APIKEY`, `VIPAYMENT_APIKEY`, `VIPAYMENT_SIGNATURE`.

Env vars consumed in code but **absent from `.env.example`**: `TRIPAY_MERCHANT_CODE/_API_KEY/_PRIVATE_KEY`, `TOKOPAY_MERCHANT_ID/_SECRET_KEY`, `DUITKU_MERCHANT_CODE/_API_KEY`, `PAYDISINI_API_KEY/_MERCHANT_ID`, `TELEGRAM_BOT_TOKEN`, `TELEGRAM_BOT_SENDER_TOKEN`, `TELEGRAM_BOT_1_TOKEN`, `TELEGRAM_BOT_2_TOKEN`, `TELEGRAM_GROUP_1_ID`, `TELEGRAM_GROUP_2_ID`, `TELEGRAM_CALLBACK_BOT_TOKEN`, `TELEGRAM_ENCRYPTION_KEY`, `TELEGRAM_API_HASH`, `ALLOW_PLAIN_TELEGRAM`. (README documents most of these.)

Unused/declared-but-idle: `REDIS_URL` (redis crate in Cargo.toml, no usage found), `TRX_INTERVAL_SECONDS` (read, never enforced — no rate limiting), `VIPAYMENT_*` (no Vipayment integration in code).

## 7. GAPS / HALF-DONE ITEMS (vs. a full store)

1. **Voucher system missing end-to-end**: `order.html` calls `POST /api/v1/voucher/check`, which is **not registered anywhere** in the router; no voucher model/table logic. Voucher field is accepted in `SubmitOrderRequest` but ignored.
2. **Reviews are decorative**: `GET /api/v1/reviews` hardcodes `[]`; product handler returns zeroed ratings; `POST /api/v1/review/submit` writes to `ulasan` table only "if present", otherwise silently discards (comments admit the table doesn't exist in this schema).
3. **Admin panel is a shell**: single-page `admin.html` with ~20 dead sidebar links; no CRUD UI for services/categories/tabs/banners/blog/providers/deposits/flashsale/vouchers; several working backend endpoints (service create, balance adjust, lock, website config, transaction status update) are unreachable from the UI. No admin sub-pages exist as files.
4. **No server-side authorization at all** on `/api/account/*` and `/api/admin/*` — identity is whatever `username` string the client sends (localStorage-based). Anyone can read/modify any account or hit admin endpoints. `users_lock` is written but never checked during login. CSRF token field exists in SubmitOrderRequest but is ignored.
5. **Deposit UX incomplete**: deposit lives only as a tab inside `account.html` (no standalone deposit page); deposit methods endpoint returns a hardcoded mock list; deposit success redirect is broken (`res.data.invoice_id` vs returned `deposit_id`); deposit invoice/return URL is a literal placeholder `https://yoursite.com/deposit/invoices/{}`; no deposit-detail page.
6. **SALDO payment flow dangling**: checkout URL points to external `https://aruterushoppu.com/order/verify-otp?...` — no local page/route serves an OTP-entry screen (only the API `POST /api/invoice/verify-otp` exists).
7. **Broken nav link**: layout.js navbar points Leaderboard to `/app/guest/leaderboard` (old PHP path) which is not a route → 404 via fallback ServeDir (only `/leaderboard` works).
8. **Product form fields empty**: `handle_product` always returns `data_form: []`, so the "Masukkan Data Akun" section of the order page renders no inputs (UserID/server fields can't be collected through this page as-is).
9. **Account "Hall of Fame" tab** is an explicit stub.
10. **Duplicate/dead code paths**: `src/routes.rs`+`main.rs` monolith duplicates a subset of store_gateway; `domain/checkers/pubg.rs` duplicates `checkers/pubg.rs`; `whatsapp/mpwa.rs` mostly unused; `grammers-client` (MTProto) is a dependency and startup logs claim MTProto init, but sender actually uses plain Bot API HTTP.
11. **Misc bugs**: `admin::transaction::update_transaction_status` filters on `web_invoice` column (schema uses `order_id`); Duitku invoice callbackUrl targets port 8081 (fulfillment opens no ports); `engine.rs` order_id format `%Y%M%S` omits day/hour; `RateLimitTriggered`/`AccountLocked` errors never raised; hardcoded personal defaults for MPWA keys/phones inside register handlers as env-var fallbacks.
12. **Present and working**: home page (index.html), full guest order→invoice→webhook→auto-fulfillment loop, auth (login/register+WA OTP/forgot), account dashboard/mutations/history/profile/API keys, pricelist, leaderboard, blog, region checker, privacy/terms, 4 payment gateway integrations with signature-verified webhooks, encrypted Telegram dual-server fulfillment architecture, cron pollers/expiry/refunds.

---
---

# LAPORAN B — FEATURE INVENTORY: PHP Webshop (aplikasi asli)

**Root:** `/Users/macmini/.mounty/SSD_External/Aruteru Shoppu NEW`
**Stack:** PHP 7.4 (alt-php74), MySQL/MariaDB (`aruy6922_bulan8new`), jQuery + Alpine.js 3 + Tailwind (guest UI), Bootstrap 4/5 + ApexCharts + DataTables (admin/account UI). URL rewriting via `.htaccess` (strip `.php`, custom routes below). Branded "Sahabat Store x DhaMus" / "Lann Digital".

## 0. Routing & Bootstrap

- `connect.php` — session start, TZ Asia/Jakarta, mysqli connect, loads `library/mainfunction.php` (helpers: `filter()`, `bcrypt()`, `currency()`, `client_ip()`, `client_iploc()` via geoplugin/ip-api, `devices()` UA detection, `RandomKey()`, `LannDie()` JSON exit, `LannResult()` flash+redirect), `library/function/csrf_token.php` (CSRFSecure class → `$csrf_string` / `$result_csrf`), `library/function/cURL.php`, `library/customfunction.php` (`conf($code,$n)` reads `conf` table; `check_lock()`; `dataCallback()/sendCallback()` reseller webhooks; `scrapeRegion()` MLBB region scraper; `qrisDynamic()` QRIS proxy; `MOBILE_LEGENDS_GLOBAL()` SmileOne checker; `pubg_checker()`), and initializes classes: `MPWA_BYLANN` (WhatsApp gateway, hardcoded token/sender), `NotificationWhatsapp`, `DuniaGames` game checkers, `DigiFlazz` (from `provider` table code=DIGI), VIPayment functions, Tripay/Paydisini/Tokopay/Duitku payment libs, cron `helper.php`. Also bot-UA blocking and page-load timer.
- `.htaccess` routes:
  - `/product/{type}/category/{slug}` → `app/guest/order/?type=&code=`
  - `/invoices` → `app/guest/invoice.php`; `/invoices/{oid}` → `app/guest/faktur?oid=`
  - `/deposit/invoices/{did}` → `app/guest/deposit/faktur?did=`
  - `/etc/{code}/product_type/{type}` → `library/ajax/check?v=&t=` (order pre-check)
  - `/lann/{code}` → `library/ajax/check-voucher?category=` (voucher check)
  - Plus 7G firewall patterns, security headers, caching, PHP 7.4 handler.

## 1. PAGES (user-facing)

### Guest storefront
| Page | File | Purpose / major sections |
|---|---|---|
| **Home** | `index.php` + `app/guest/home.php`, `app/guest/app.php`, `app/guest/popup.php`, `app/guest/flashsale.php` | Cookie-based auto-login (`users_cookie` token+ssid) before render. Sections: hero banner swiper (from `banner` table, bubbles/meteor animations); FLASHSALE marquee w/ countdown timer (`flashsale` table, % OFF badges, progress bar); TRENDING popular categories grid (`category.popular='1'`); tabbed category browser (`tabs` table drives tabs, 12 categories per tab + AJAX "Tampilkan Lainnya…" load-more via `library/ajax/games-loaded.php`); "Berita Terkini" blog cards (`blog` table); announcement popup modal (image/title/content from `conf popup`, "don't show again" in localStorage). |
| **Guest layout** | `library/layout/guest/header.php`, `footer.php` | Navbar: logo/img toggle, search input opening Alpine search modal (AJAX `searching-game`), links: Beranda `/`, Cek Transaksi `/invoices`, Cek Region `/page/region`, Harga `/page/pricelist`, Leaderboard `/app/guest/leaderboard`; theme toggle (dark/light via localStorage `data-theme`); Masuk or Dashboard button. Mobile hamburger menu duplicates links. Footer: wave/footer image (`conf footer`), logo+description, link grids (Peta Situs, Legalitas→privacy/terms, Dukungan→social links from `conf social`, Autentikasi), address + copyright, floating CS mascot widget (WhatsApp/Email options), toast system, tab-switcher JS, result alerts include. |
| **Product order page** | `app/guest/order/index.php` (+ `system.php`, `content.php`, `payment*.php`, `voucher.php`, `testimoni-*.php`) | Product hero banner/thumbnail/name/owner/trust badges. Step 1 "Masukkan Data Akun": dynamic inputs generated from `category.data_form` JSON (text/number/email/server dropdown fields), target note. Step 2 "Pilih Nominal": skeleton loader then variant filter buttons (`varian` table) with Alpine filtering; product radio-cards from `service` (price = base + tamu/member/reseller profit; flashsale discount shown with strikethrough + %OFF ribbon). Confirmation modal (Tujuan/Nickname/Produk/Item/Harga/Pembayaran) before submit. Step 3 "Pilih Metode Pembayaran": Saldo Akun (APP, login-only, BEST PRICE badge), QRIS methods, E-Wallet accordion, Virtual Account accordion, Convenience Store accordion (all from `metode` table, per-method fee display updated by AJAX `price`). Step 4 "Kode Promo" voucher input + check button (AJAX `/lann/{code}`). Step 5 WhatsApp number input (validated 08xx, saved to localStorage). "Pesan Sekarang" submits form → POST handled by `library/082132175370.app/order-action.php`. Left column: service guarantees, collapsible purchase notes (`category.note` base64), app download links if `category.apk`, desktop testimonials widget (avg rating, star distribution bars, latest reviews from `ulasan`). Mobile testimonial section too. |
| **Invoice finder** | `app/guest/invoice.php` | Search form (order id + CSRF) redirecting to faktur; "Transaksi Real-time" table of last 10 transactions (date, masked invoice no, target, price incl fee+uniq, status badge; note 'Gagal melakukan pembayaran' displayed as Expired). |
| **Faktur / invoice detail** | `app/guest/faktur.php` | Payment bill page for one order: product image/category/service, ID + copy, nickname (base64-decoded), Region (via `regionName()` scrape), invoice number + copy, status badges (transaction status & payment status color-coded), payment action block (QRIS image + download, VA number + copy, bank account "NO AN NAME", or "Bayar Sekarang" pay_url link), Rincian Pembayaran disclosure (fee as tax, uniq, method, total), transaction progress timeline (created/paid/processed/error/system states), QRIS info alert, success note alert, review submission form (5-star picker + message, only when success & not yet reviewed → INSERT into `ulasan`), print CSS + `print_invoice()`, 2-second polling AJAX `status-loaded` auto-refresh on status change. |
| **Leaderboard** | `app/guest/leaderboard.php` | Top buyers: Daily Top 3, Weekly Top 5, Monthly Top 10 boxes (gold/silver/bronze styling, verified ✓, anonymous fallback 'Anonim'), aggregated from successful `transaction` sums. |
| **Blog detail** | `app/guest/blog.php` | Single article by slug title: banner image, title, base64-decoded content. |
| **Region checker** | `page/region.php` | MLBB region checker form (User ID + Server) → AJAX `stalk-ml` → shows username/region/server result card. |
| **Pricelist** | `page/pricelist.php` | Category select + refresh button; table Code/Name/Harga Tamu/Member/Reseller/Status loaded via AJAX `pricelist` (flashsale-discounted prices). |
| **Reviews** | `page/review.php` | Testimonial card grid from `ulasan` (category icon, star rating, message, masked order id, service name, date) with client-side load-more (10/page). |
| **Privacy / Terms** | `page/privacy-policy.php`, `page/terms-and-condition.php` | Static pages rendering `conf('pages',1)` / `conf('pages',2)` content. |
| **Location lock screen** | `page/location.php` | Standalone "Terkunci" 404-style page telling user to enable location (used when location requirement is on). |
| **Deposit bill** | `app/guest/deposit/faktur.php` | Deposit invoice: deposit ID + copy, payment status badge, payment action (QRIS image/download, VA number, bank account, pay link), Rincian (method/request/fee/uniq/sub-total), progress timeline (created/refund/cancel/paid), QRIS hint, 2s polling refresh. |

### Auth pages (`auth/`)
- `login.php` — username/password form, show/hide password, remember-me checkbox, Cloudflare Turnstile OR Google reCAPTCHA (per `conf captcha`), optional background image (`conf auth`), register link, forgot-password link. Handler `library/082132175370.app/auth-login.php`: captcha verify, suspend/inactive checks, bcrypt verify, maintenance/lock checks, sets `ssid`/`token` cookies + `users_cookie` row + `users.sso`, logs to `logs`, sends WhatsApp login-alert (IP geolocation via ipinfo.io) per level config.
- `register.php` — username/email(whitelist gmail/yahoo/outlook/icloud)/phone/password+confirm + captcha; OTP step (6-digit sent via WhatsApp using template `conf notification,1` with `{{ OTP }}`), resend with 120s cooldown, cancel. Creates `users` (Member/active), `users_api` row, log entry, admin WhatsApp registration alert.
- `forgot-password.php` — reset by phone number: generates random password, updates `users`, sends new password via MPWA WhatsApp, logs 'reset-password'.
- `logout.php` — logs logout, deletes `users_cookie` row, clears cookies/session.
- `header.php`/`layout.php`/`result.php` — auth shell + flash messages.

### Account area (member dashboard, layout `library/layout/sess/header.php` + `sidebar.php`)
- `account/index.php` — Dashboard: welcome card w/ day-over-day order congrats message, Balance / Total Order / Total Deposit stat cards, Order Revenue & Deposit Revenue this-vs-last month + ApexCharts 8-day line charts.
- `account/profile.php` — Tabs: Detail (gravatar avatar, balance, email, WA, level, last login), Change Name (name + phone update), Change Password (old/new/retype), API Setting (API ID + regenerate, API KEY + regenerate, signature md5(uid.key)+copy, whitelist IP, callback URL, dev/prod status). Handler `library/082132175370.app/account-settings.php`.
- `account/history.php` — Order History DataTable (server-side `account/table/order.php`): date, order id link to invoice, category/service, price, serial-number (note) with copy.
- `account/deposit/new.php` — Deposit form: payment type radios (E-Wallet/Virtual/Convenience) → AJAX `account/deposit/ajax.php?type=payment` fills method select (fee labels); amount field enabled after method fetch showing min/max note; submit → `deposit-action.php`.
- `account/deposit/history.php` — Deposit history DataTable (`account/table/deposit.php`).
- `account/mutation.php` — Balance mutation history DataTable (`account/table/mutation.php`): date, note, ±amount badge.
- `account/activity.php` — Activity log DataTable (`account/table/activity.php`): date, IP/address, note (from `logs`).
- `account/page/hall-of-fame.php` — Hall of Fame (gated by `conf xtra-fitur,3`): TOP 5 Today, TOP 5 This Month (users), TOP 10 Service this month.
- `account/page/privacy.php`, `question.php`, `terms.php` — static/legal/FAQ pages.

### Misc root files
- `quick-orders.php`, `simple-orders.php`, `quick-stats.php` — debug/quick-fix JSON endpoints for the manual-orders admin widget (paged provider='X' orders HTML rows, filters date/status/order_id, stats counts incl. `notification_logs`).
- `a.html` — standalone AJAX test harness for `admin/others/ajax/manual-orders.php`.
- `print.php` — dev script testing `scrapeRegion()` for MLBB region.
- `region.txt` — placeholder text file.

## 2. ADMIN PANEL

Auth gate `library/session/admin.php` (requires `level == 'Admin'`); layout `library/layout/sess/header.admin.php` + `sidebar.admin.php` (menu exactly as listed below); shared modal loader `library/layout/sess/modal.php`; server-side DataTables helper `library/function/ssp.class.php` + `datatable()` JS generator (auto-refresh interval option).

### admin/dashboard/
| File | Feature |
|---|---|
| `devices.php` | Read-only analytics: Session By Devices, Device Statistics, Browser Statistics charts (from `users_cookie`/UA data). |
| `statistics.php` | Stat cards Total Users / Total Balance / Total Deposit + 7-day sparkline area charts (registrations, mutations, deposits). |
| `renevue.php` | Revenue cards (total revenue+trx count, total profit) + This-month/Last-month revenue charts for orders and deposits. |

### admin/users/
| File | Feature |
|---|---|
| `manage.php` | Users list DataTable (`table.php?__s=1`): status icon (active/suspended/locked), email, phone, balance, registered date; actions Detail / Edit modals (+ Location modal if `xtra-fitur,2`). Add User modal (`modal/add.php` → `admin-users-add.php`: create user w/ bcrypt, level, balance). |
| `modal/detail.php` | Read-only user detail modal. |
| `modal/edit.php` | Edit user: general (email/phone/level/status), change password, send balance (+amount+reason → mutation), cut balance (−amount+reason → mutation), lock/unlock account (`users_edit_locked` → `users_lock`). Handlers in `admin-users-edit.php`. |
| `modal/location.php` | User GPS-location map modal (Leaflet iframe). |
| `locked.php` | Locked accounts DataTable (`table.php?__s=2`, `users_lock`); unlock action GET `?unlock=` deletes row (`admin-users-locked.php`). |
| `mutation.php` | All-users mutation ledger DataTable (`table.php?__s=3`, `mutation`). |
| `activity.php` | All-users activity log DataTable (`table.php?__s=4`, `logs` where user≠system). |

### admin/config/
| File | Feature |
|---|---|
| `website.php` | Site configuration forms (handler `admin-config-website.php`): General (title, navbar text, description, keyword, banner, icon, image-mode toggle, navbar image), Footer (mode/image), UI category mode toggle, Captcha (Cloudflare/Google keys), Auth (remember seconds, background), Meta/Tag manager (Bing, Google site verification, GTM/head scripts base64). |
| `pages.php` | Terms & Privacy editors (CKEditor textareas → `conf pages,1/2`). |
| `general/index.php` + `edit.php` + `table.php` | FAQ CRUD ("General Questions"): add quest/answer, edit modal, delete, DataTable (`conf`-backed FAQ rows). |
| `prefix.php` | Prefix settings: transaction-ID prefix, deposit-ID prefix, hold-seconds between transactions, auto-profit-update switch (`prefix,4`), apply-flashsale-to-API switch (`prefix,5`). |
| `others.php` | Extra Features toggles (SSO device, require location, hall-of-fame) and Maintenance toggles (website/transaction/deposit/register) — instant AJAX save. Shows server IP. |
| `img/example.php` | AJAX snippet returning example image (help for global image form). |

### admin/others/
| File | Feature |
|---|---|
| `banner.php` | Banner CRUD: multi-link add form (`others_addbanner`), manage DataTable (`others/table.php?__s=1`), delete. |
| `social.php` | Social links form: WhatsApp, Instagram, Tiktok, Facebook (+email used in footer) → `conf social`. |
| `color.php` | Theme colors form: Main/Left/Right/Body → `conf color` (injected as CSS vars in guest header). |
| `notification.php` | WhatsApp notification templates (base64 in `conf notification`): Register OTP (`{{ OTP }}`), Transaction (12 placeholders), Deposit (9 placeholders), Manual Order (template + link to manual list). |
| `notification-manual.php` | Manual-order notifications dashboard: filter bar (date/status/order-id), stat cards (Total Manual/Pending/Success/Notif Terkirim), paged table of provider='X' transactions with notification status and actions (detail modal, resend WA, update status). Powered by `ajax/manual-orders.php`. |
| `popup.php` | Popup announcement form: image, subject, content, Show/hide → `conf popup`. |
| `blog/index.php` + `edit.php` + `table.php` | Blog CRUD: add (banner URL, title, CKEditor content base64), edit modal, delete, DataTable. |
| `ajax/manual-orders.php` | JSON AJAX: get_orders (paged/filterable provider X list joined to `notification_logs`), get_statistics, get_detail, resend_notification. |

### admin/tabs/
- `index.php` — drag-and-drop tab ordering (dragula) + Add/Edit/Delete modals (`modal/add|edit|delete.php`); handlers `admin-tabs.php` (tabs_add/tabs_edit/tabs_delete on `tabs` table).
- `ajax.php` — persists new order of tab IDs.

### admin/category/
- `index.php` — Category DataTable (`table.php`); Menu: Add (`modal/add.php`), Clear all/by-type (`modal/clear.php`), Populer manager link.
- Handler `admin-category.php`: category_add, category_clear, savecty (owner/checker/note/type/popular/apk flags), saveimg (image/banner/profile JSON), savenote, prefix_add, data-form field add (`category_add_input` builds `data_form` JSON with text/number/email/server options), data-form field delete (GET del), delcty (delete category + optionally its services).
- `part.php` — per-category dynamic form builder UI (add/remove input fields incl. server dropdown value pairs).
- `note.php` — purchase-note editor (base64).
- `populer.php` — Popular/trending manager: set popular flag/order, remove, reset ordering.
- `ajax.php` — reorder `varian` rows; `ajax-populer.php` — popular ordering AJAX.

### admin/service/
- `index.php` — Service DataTable (`table.php` via SSP: name/status/prices 4-tier/provider/actions Detail/Edit/Delete). Menu: Add (`modal/add.php`), Clear (`modal/clear.php` bulk delete by category/provider/status), Set Global Profit/Image/Part/Type links.
- Handler `admin-service.php`: service_add, service_edit (name, price, tamu/member/reseller margins, status, image, varian sub), service_clear, GET del.
- Global tools (handlers in file or `admin-service-global.php`): `profit.php` (global profit by fixed/range/per-category; AJAX `ajax-profit.php` preview), `image.php` (bulk set product images; `ajax-image.php` lists services), `part.php` (bulk set `sub`/varian; `ajax-part.php`), `type.php` (bulk set type; `ajax-service.php` lists services of category), `ajax.php` (varian ordering).
- `modal/detail.php` — read-only service detail.

### admin/provider/
- `index.php` — Provider DataTable (`table.php`): name/code/balance/actions Edit + Check.
- `edit.php` — Modal editing credentials per provider (labels adapt: DIGI Username/Production Key, VIP API ID/Key, FONNTE token, TRIPAY Secret/API, PAYDISINI, TOKOPAY, DUITKU merchant/mode, KEYPEDIA, QR_LANN, X/manual). Handler `admin-provider.php` (provedit).
- `check.php` — AJAX connectivity test (`checkProvider()` returns balance etc.).

### admin/transaction/
| File | Feature |
|---|---|
| `manage.php` | Status count cards (Error/Pending/Success/Process/System) + all-transactions DataTable (`table.php`) with per-row status dropdowns calling `status.php` (order status) and `status-payment.php` (bill status), Detail/Edit modals. |
| `status.php` | AJAX set order status (error/pending/process/system/success); 'system'+'process' triggers re-processing via `status-transaction-action.php`; success/error fire `trxNotification()` + `dataCallback()`. |
| `status-payment.php` | AJAX set payment status (paid/unpaid/cancel/refund). |
| `modal/detail.php` | Tabbed detail: Payment (gateway rid, method, voucher/flashsale breakdown, fee/uniq/total, expiry), Transaction (order/user/WA/target/nick/note/status/source WEB/API/LINE/WA/Telegram/dates), Provider (tid/output JSON), Review (rating/message). |
| `modal/edit.php` | Edit transaction note. |
| `report.php` | Financial report: date-range form; totals table (orders, gross income Σprice, net Σprofit); per-provider breakdown; ApexCharts daily profit line (2–31 days); `report-detail.php` modal. |
| `voucher.php` + `voucher-table.php` + `modal/voucher-add.php` | Voucher CRUD on `voucher` (category, code, discount ≥100, minimum, stock); delete via GET. |
| `flashsale.php` + `flashsale-table.php` + `modal/flashsale-add.php` + `flashsale-status.php` | Flashsale management: global end-time datetime (`send_timexp` → `conf flashsale,1`), add discount per service (via `ajax/flash-category.php` + `ajax/flash-service.php` cascading selects), toggle active status, DataTable. Handler `admin-flashsale.php`. |
| `order-actions.php` | JSON admin API for manual orders: get_detail / update_status (logs to `activity`, sends WA status update) / resend_notification. |

### admin/financial/
- `sales.php` — Sales report table (transactions grouped summary).
- `profit.php` — Profit report.
- `summary.php` — Summary by Status and by Users tables. (All read-only aggregations over `transaction`.)

### admin/deposit/
| File | Feature |
|---|---|
| `manage.php` | Deposit requests DataTable (`table.php?__s=2`): user/method/amount/date + Unpaid dropdown → Paid/Cancel via `status-deposit.php` (manual override; Paid credits balance elsewhere—cron does it automatically). |
| `method.php` | Payment-method CRUD on `metode` (`table.php?__s=1`): Add/Edit/Detail/Delete modals; handler `admin-method.php` (provider, code, type ewallet/virtual/convenience/app, fee_type +/-, percent, flat, fee_using pelanggan/merchant, min/max, expired minutes, note/guide, image, status on/off). |
| `report.php` | Deposit report: date range + status filter; aggregated per payment method & account number (requests, amount, fee) + grand totals. |
| `modal/add|edit|detail.php` | Method forms. |

### admin/server/
- `cronsjob.php` — Read-only reference page listing curl commands for crons (order_expired, deposit_expired, order_refund, flash_expired, status-process, status-system), callback URLs (Digiflazz, VIPayment, TRIPAY, PAYDISINI, TOKOPAY, DUITKU), and Get-Service URLs (DIGIFLAZZ/VIPayment/KEYPEDIA) with copy buttons.
- `error-log.php` + `table.php` — System error log DataTable (`logs` where user='system' AND type='system').

## 3. API ENDPOINTS

### Public/reseller JSON API (`api/*.php`, POST form-data, auth `key`+`sign`=md5(uid.ukey), IP whitelist from `users_api.whitelist`, maintenance-gated)
| Endpoint | Params | Response | Behavior |
|---|---|---|---|
| `api/profile.php` | key, sign | `{result, data:{username,balance,level,registered}, message}` | Account details. |
| `api/service.php` | key, sign, filter_type(type|brand), filter_value, filter_status | `{result, data:[{code,category,name,type,price:{guest,member,reseller},varian,status,update_at}], message}` | Service price list (guest/member/reseller totals). |
| `api/order.php` | key, sign, service (code), target (`id\|zone`) | `{result, data:{order_id,data,code,service,status:'process',note:'Berhasil dibayar',price}, message}` | Balance-paid order: validates balance/hold/anti-spam, inserts transaction (payment APP/paid), calls `api/order-action.php` (DIGI/VIP/KEYPEDIA/X topup), deducts balance + mutation, records flashsale if `prefix,5` enabled, deletes row on failure + logs. |
| `api/status.php` | key, sign, order_id?, limit? | `{result, data:[{order_id,data,code,service,status,note,price}], message}` | Transaction status (own orders only). |
| `api/order-action.php` | (internal include) | sets `$req_result/$req_order_tid/$req_status` | Provider dispatch DIGI→`$DIGI->Topup`, VIP→`vipayment_createTransaction`, KEYPEDIA→POST {link}order, X→instant process. |

### Web/AJAX endpoints (`library/ajax/`)
| Endpoint | Purpose |
|---|---|
| `check.php` (route `/etc/{code}/product_type/{type}`) | Order pre-check: validates product/method/voucher/flashsale, runs game-account checker (MLBB SmileOne/DuniaGames/PLN inquiry/pascabayar inquiry), returns JSON `{result:{status:200}, product, items, price, pembayaran, nickname(+region)}` or `error_msg`; also `bill_service` variant for pascabayar bills. |
| `check-voucher.php` (route `/lann/{code}`) | Voucher validation: exists+stock, not on flashsale item, minimum spend → `{status,message}`. |
| `price.php` | Per-product price for every active method incl. fees/min/max → array `[{code,price,result}]`. |
| `method.php` | Validate chosen method against product price (min/max). |
| `pricelist.php` | Pricelist table rows HTML (3-tier prices, flashsale-adjusted, status badge). |
| `searching-game.php` | Live search categories LIKE, returns `<li>` results HTML (limit 15). |
| `games-loaded.php` | Home load-more: next 12 categories HTML for a tab. |
| `stalk-ml.php` | MLBB region scrape → `"Nickname (Region)"` or `lann_scrape_system`. |
| `status-loaded.php` | Polling: transaction `{status_payment,status}` or deposit `{status}` by base64 id. |
| `iframe/users-location.php` | Leaflet map iframe: all users' locations (__v=1), one user (__v=2, admin), own cookie coor (__v=3). Hidden backdoor-ish branch: with `start` bcrypt param allows balance/order calls to DIGI/VIP. |

### Admin AJAX (see §2)
`admin/category/ajax.php`, `ajax-populer.php`; `admin/service/ajax.php`, `ajax-service.php`, `ajax-image.php`, `ajax-part.php`, `ajax-profit.php`; `admin/tabs/ajax.php`; `admin/transaction/ajax/flash-category.php`, `flash-service.php`; `admin/transaction/status.php`, `status-payment.php`, `flashsale-status.php`, `order-actions.php`; `admin/deposit/status-deposit.php`, `table.php`, `ajax` none; `admin/users/table.php`; `admin/server/table.php`; `admin/others/table.php`, `blog/table.php`, `config/general/table.php`, `provider/table.php`, `service/table.php`, `category/table.php`, `transaction/table.php`, `voucher-table.php`, `flashsale-table.php` (all SSP DataTable feeds); `admin/others/ajax/manual-orders.php`; `admin/provider/edit.php`, `check.php`; modals under `*/modal/*` (AJAX-loaded fragments).

### Account AJAX
`account/deposit/ajax.php` (payment-type → method options; method → min/max note); `account/table/order.php`, `deposit.php`, `mutation.php`, `activity.php` (SSP feeds).

## 4. DATABASE SCHEMA (tables inferred from queries)

| Table | Key columns (as used) |
|---|---|
| `users` | id, username, email, phone, name, password(bcrypt), balance, level(Member/Reseller/Admin), status(active/suspend), date, sso |
| `users_api` | user, uid, ukey, whitelist(IP csv), callback(URL), status(development/production) |
| `users_cookie` | cookie(token), token(FCM), username, date_cr?, active, expired, ua, ud(device), ip, loc, dev(os), coor, browser |
| `users_lock` | user, reason, date_cr(date) |
| `users_location` | id, user, lon, lat, ua, ud, ip, loc, dev, total, date_cr, date_up |
| `category` | id, code, name, order, popular, count_form, data_form(JSON), prefix, image(JSON image/banner/profile), apk, note, target_note, code_checker, owner, type(games/pulsa-reguler/...), status? ('false') |
| `service` | id, code, type, sub(varian), image, game(category code), name, price(base), tamu, member, reseller(margins), status(available/empty), provider(DIGI/VIP/KEYPEDIA/X), date_up |
| `varian` | id, order, code(category), name, type |
| `tabs` | id, name, icon, type, order |
| `transaction` | id, order_id, order_tid(provider trxid), payment_rid(gateway ref), user, code, payment_code, payment_type, metode(name), category(game), name(service), data(target), nickname(base64), field(form spec), phone, payment_action(QR/VA/url/note), price, fee, uniq, profit, status(pending/process/success/error/system), status_payment(unpaid/paid/cancel/refund), voucher(`code###discount###oldprice`), flashsale(`amount###oldprice###id`), date_cr, date_up, expired_at, provider, payment_provider(APP/TRIPAY/PAYDISINI/TOKOPAY/DUITKU/QR_LANN/X), payment_guide, output(JSON), type, refund(0/1), trxfrom("WEB,ip"/"API,ip"), callback(url) |
| `deposit` | id, deposit_id, deposit_pid, user, payment(method code), type, method(name), status(unpaid/paid/cancel/refund), amount, fee, uniq, note, action(payment payload), provider, guide?, date, expired_at |
| `mutation` | id, user, type(+/−), amount, note, date |
| `ulasan` (review) | id, user, order_id, category, name(service), rating(1-5), message, date |
| `voucher` | id, code(category), voucher(code), discount, minimum, stock, date_cr |
| `flashsale` | id, code(service), amount(discount), status(0/1) |
| `banner` | id, content(image URL) |
| `blog` | id, title, banner, content(base64), date_cr |
| `logs` | id, user, type(login/logout/register/reset-password/system), text, ip, loc, date |
| `notification_logs` | order_id, phone, message, status(success/failed), created_at |
| `provider` | id?, code(DIGI/VIP/KEYPEDIA/FONNTE/TRIPAY/PAYDISINI/TOKOPAY/DUITKU/QR_LANN/X), userid, apikey, merchant?, link, mode(sandbox/production), type(transaction/payment), name |
| `metode` | id, code, name, type(app/ewallet/virtual/convenience), provider(APP/TRIPAY/PAYDISINI/TOKOPAY/DUITKU/QR_LANN/X), fee_type(+ or percent), percent, flat, fee_using(pelanggan/merchant), min, max, expired(minutes), note, guide, image, status(on/off) |
| `conf` | code, c1..cN columns — codes seen: webcfg(1-8), hold-balance(1-3), webmt(1-4), tag_manajer(1-5), captcha(1-5), xtra-fitur(1-3), prefix(1-5), profit(1-4), flashsale(1), popup(1-4), color(1-4), social(1-5), footer(1-2), notification(1-4), pages(1-2), auth(1-2), ui_category(1) |
| `activity` | user, action, data(JSON), date, ip (written by order-actions.php) |

## 5. PAYMENTS

**Gateways (all four present):**
- **Tripay** (`library/function/payment/Tripay.php`): channelPayment, requestTransaction, detailTransaction, paymentInstructions; creds from `provider` TRIPAY; HMAC-SHA256 callback verify (`cron/.../tripay/callback.php`).
- **Paydisini** (`Paydisini.php`): channelPayment, request/detail/paymentInstructions; callback verifies apikey + md5(apikey.order_id.'CallbackStatus') + fixed IP 194.233.92.170 (`paydisini/callback.php`).
- **Tokopay** (`Tokopay.php`): request/detail transaction; callback `tokopay/callback.php`.
- **Duitku** (`Duitku.php` class): getPayment, getStatus, createTransaction (sandbox/production via `provider.mode`); callback `duitku/callback.php`; writes `duitku.json` debug.
- **QRIS custom "QR_LANN"**: dynamic QRIS via external proxy `qrisDynamic()` → `sahabatstoretopup.com/.../forward/qris-proxy.php` (local forwarder stub `forward/qris.php` + qris.txt); adds unique-code `uniq=rand(1,150)` for matching.
- **X (manual)**: stores instruction text (e.g., "12345 AN NAME"), pending until admin marks paid.
- **APP (saldo)**: direct balance deduction.

**Order payment flow** (`library/082132175370.app/order-action.php` + `-curl.php`):
1. Validate CSRF, maintenance, product/method/provider, target, min/max, WA phone, lock, saldo sufficiency + per-level hold balance, anti-spam (hold seconds via last_trx), duplicate pending transaction on same target, voucher vs flashsale exclusivity.
2. Price = base + margin(tamu/member/reseller) − voucher.discount − flashsale.amount; fee from `metode` (flat or %+flat, waived if fee_using=merchant); uniq for QRIS/X.
3. Insert `transaction` (pending/unpaid, expired_at = now + method.expired minutes, trxfrom="WEB,ip").
4. Dispatch by `payment_provider`: APP→provider topup immediately (DIGI/VIP/KEYPEDIA/X) and mark paid+process, deduct balance + mutation; TRIPAY/PAYDISINI/TOKOPAY/DUITKU/QR_LANN→create invoice, store payment_action (QR img / VA / pay_url / bank no), stay unpaid; X→pending with note.
5. On failure: delete transaction + insert system log. On success: store voucher/flashsale snapshot columns; manual (X) orders trigger `NotificationWhatsapp::sendManualTransactionNotification()` to admin WA; redirect to `/invoices/{oid}`.

**Deposit flow** (`account/deposit/new.php` → `deposit-action.php` + `deposit-action-curl.php`): same pattern with deposit_id prefix, min/max, fee/uniq, creates `deposit` row (unpaid) and gateway invoice; redirect `/deposit/invoices/{did}`. Note: Duitku branch has a bug (`$method_provided`).

**Saldo payment**: `payment_provider='APP'` — checks balance ≥ price and ≥ price+hold[level]; deducts `users.balance`, inserts negative `mutation`.

**Callbacks & settlement (fulfillment)** — see §6.

## 6. FULFILLMENT

Providers: **Digiflazz** (`library/function/order/Digiflazz.php`: CheckBalance, PriceList prepaid+pasca, Topup, CheckTopup, CheckPasca, CheckBill, PayBill, InquiryPLN; sign=md5(user.key.ref)), **VIPayment** (`VIPayment.php`: profile/createTransaction/checkStatus/getService), **KEYPEDIA** (generic curl to provider.link order/status), **X manual**.

Mechanisms:
1. **Synchronous on paid** — APP-saldo and API orders call provider immediately (`order-action-curl.php`, `api/order-action.php`).
2. **Webhooks** — `library/cron/fe_bydm082132175370/{digiflazz,vipayment}/callback.php`: Digiflazz validates headers (`x-digiflazz-event`, UA Digiflazz-Hookshot, x-hub-signature presence), maps status via `helper::filter_status`, updates transaction, fires `trxNotification()` (customer WA) + `dataCallback()` (reseller webhook to `users_api.callback` with IP whitelist) on success/error.
3. **Payment callbacks** — `tripay/callback.php`, `paydisini/callback.php`, `tokopay/callback.php`, `duitku/callback.php`: on paid → mark paid and immediately trigger provider order via shared `status-transaction-action.php`; on cancel/refund → status error, profit=0.
4. **Pull crons** (curl jobs listed in admin/server/cronsjob.php):
   - `sistem.php?req=order_expired` — expire unpaid orders past `expired_at` (status error, payment cancel, restock voucher).
   - `?req=deposit_expired` — cancel expired unpaid deposits.
   - `?req=order_refund` — refund paid-but-error APP/X orders to balance + mutation, set refund=1.
   - `?req=flash_expired` — deactivate flashsale past `conf flashsale,1` time.
   - `status-process.php` — poll providers (VIP/DIGI incl. pasca/KEYPEDIA) for 'process' orders, update status, notify+callback on success.
   - `status-system.php` — re-check 'system'-status orders against payment gateways; if actually paid, retry provider dispatch.
   - `status-deposit.php` — poll gateways for unpaid deposits; on paid credit `balance += amount+uniq`, mutation, `depoNotification()`.
5. **Catalog sync crons** — `digiflazz/services.php`, `vipayment/services.php`, `keypedia/services.php`: pull price lists, auto-create missing `category`/`varian`(default "✨ Instant")/`service` rows, update price/status and (if `prefix,4`) auto margins from `conf profit` (fixed or multiplier per tier); brand-name cleanup via `lanndigitalReplace()`; type mapping via `helper::filter_type()`.

## 7. FEATURES LIST

### USER-FACING
1. Guest checkout without account (WhatsApp-number based) — `app/guest/order/*`
2. Member/reseller tiered pricing (tamu/member/reseller margins) everywhere
3. Flashsale pricing + countdown + %OFF ribbons + marquee (`flashsale` table, home + order + pricelist)
4. Voucher/promo codes at checkout with stock decrement, minimum spend, per-category scope, restock on expiry (`voucher`, `check-voucher.php`, order-action)
5. Dynamic per-game account forms (data_form builder: text/number/email/server dropdowns) + target notes
6. Game-account checkers/nickname lookup: MLBB (SmileOne + DuniaGames region scrape), PUBG, Free Fire, COD, AoV, LifeAfter, Speed Drifters, Point Blank, Marvel Super War, Lokapala, PLN inquiry, pascabayar bill inquiry (`GameChecker.php`, `customfunction.php`, `check.php`)
7. MLBB region checker page (`page/region.php`, `stalk-ml.php`, `scrapeRegion()`)
8. Reviews/testimonials: star-distribution widget on product page, submit on faktur after success, testimonials page with load-more (`ulasan`)
9. Leaderboard page (daily/weekly/monthly top buyers) + member Hall-of-Fame page (top users/services)
10. Invoice finder + real-time recent transactions feed; faktur page with print stylesheet, copy buttons, QR download, progress timeline, auto-refresh polling
11. Blog/news (list on home, detail page)
12. Banner carousel, announcement popup (dismiss persisted), trending/popular categories, tabbed catalog with lazy load-more, live search modal
13. Dark/light theme toggle (localStorage), custom theme colors from admin
14. Pricelist page (3-tier prices, flashsale-adjusted)
15. Deposits: multi-gateway top-up (ewallet/virtual/convenience), fee display, deposit invoices, history
16. Saldo (balance) payments with per-level hold balance and anti-spam throttle
17. Profile: edit name/phone, change password, gravatar avatar, last login
18. API keys management: uid/ukey regeneration, signature display, whitelist IPs, callback URL, dev/prod status
19. Activity log & mutation history pages; order history with SN copy
20. Notifications: WhatsApp OTP at register, transaction & deposit WA notifications (templated), login-security WA alerts, forgot-password via WA
21. Auth: register w/ WA OTP + cooldown, login w/ Turnstile/reCAPTCHA, remember-me cookie SSO, single-session enforcement option, account locking, suspension, maintenance modes
22. Reseller API (profile/service/order/status) with callbacks (`dataCallback`)
23. Optional GPS/device tracking collection (`track-gps.php`, `track-cookie.php`, geoDevices.js) + location-required gate
24. CSRF protection on all forms, input filtering, 7G firewall, bot-UA blocking

### ADMIN
1. Dashboard: devices/browser stats, platform statistics cards+charts, revenue dashboards
2. Users: CRUD, detail, balance send/cut (with mutations), password reset, level/status change, lock/unlock w/ reason, locked list, global mutation ledger, activity viewer, GPS location map
3. Category CRUD + clear-all/by-type + popular/trending manager + per-category form builder + notes + prefix + images + APK links + varian ordering
4. Service CRUD + bulk clear + global profit/image/part/varian/type tools + per-row 4-tier pricing view
5. Tabs manager (drag-order)
6. Provider credential management + connectivity check (balance)
7. Transactions: manage table w/ status & payment-status overrides, reprocess 'system' orders, detail modal (payment/trx/provider/review tabs), note edit, financial report w/ charts + per-provider breakdown, report detail
8. Voucher CRUD; Flashsale manager (items + global expiry)
9. Financial reports: sales, profit, summary by status/by user
10. Deposits: manage (manual paid/cancel), method CRUD (fees/min/max/expiry), deposit report by method/account
11. Others: banners, blog CRUD, popup, social links, theme colors, WA notification templates (register/trx/deposit/manual)
12. Manual-order center: filtered list, stats, detail, status update, resend WhatsApp notification (`notification-manual.php`, `manual-orders.php`, `order-actions.php`, quick-* helpers)
13. Server: cronsjob command/URL reference page, system error-log viewer
14. Get Layanan: one-click catalog sync from DIGIFLAZZ / VIPAYMENT / KEYPEDIA (sidebar buttons hitting the services crons)
15. Config: website meta/SEO/tag-manager, footer, UI mode, captcha keys, auth settings, legal pages (CKEditor), FAQ CRUD, prefixes/holds/auto-profit/flash-for-API switches, extra features toggles, maintenance toggles (site/trx/deposit/register)

### Notable quirks worth porting decisions
- Hardcoded WhatsApp MPWA token/sender in `connect.php` and `forgot-password.php`; admin numbers hardcoded in `auth-login.php`/`auth-register.php`/`NotificationWhatsapp.php`.
- `admin/transaction/order-actions.php` references non-standard tables/columns (`user`, `activity`, `ket`, `service` numeric) — partially broken legacy code.
- `deposit-action-curl.php` Duitku branch typo (`$method_provided`), several typos (`$see_category` undefined in status processors, `JSON_PRETTRY_PRINT` in digiflazz callback).
- `DOCUMENTATION.md` confirms the Rust rewrite targets a *reduced* schema (no `ulasan`, `metode`, `voucher`, `conf`, `varian`, `users_location`, `notification_logs`, `logs`, `activity` handling) — this inventory enumerates everything the PHP original actually uses so gaps can be compared feature-by-feature.

---

## CATATAN PEMBANDING CEPAT (ringkasan gap utama Rust vs PHP)

| # | Fitur PHP | Status Rust |
|---|---|---|
| 1 | Voucher checkout + CRUD admin | ❌ Tidak ada end-to-end (endpoint `/api/v1/voucher/check` dipanggil UI tapi tak terdaftar) |
| 2 | Reviews/ulasan (list, rating produk, submit) | ⚠️ UI ada, backend selalu kosong |
| 3 | data_form dinamis per kategori (form Step 1) | ❌ Selalu `[]` |
| 4 | Deposit invoice page `/deposit/invoices/{id}` | ❌ Tidak ada halaman; redirect rusak |
| 5 | Halaman OTP saldo `/order/verify-otp` | ❌ Tidak ada (API ada) |
| 6 | Hall of Fame member | ⚠️ Stub kosong |
| 7 | Admin panel lengkap (~15 modul CRUD) | ⚠️ SPA shell, hanya 3–4 pane jalan |
| 8 | Auth server-side untuk /account & /admin API | ❌ Tidak ada (client-side saja) |
| 9 | users_lock dicek saat login | ❌ Ditulis tapi tak pernah dicek |
| 10 | External API profile + order | ⚠️ Hanya service & status yang ada |
| 11 | Flashsale global expiry (`conf flashsale,1`) | ⚠️ Waktu dibaca tapi tidak dipakai mem-filter |
| 12 | WA notification templates (register/trx/deposit/manual) | ⚠️ Sebagian hard-coded, tidak bisa dikelola admin |
| 13 | Get Layanan sync (DIGIFLAZZ/VIPAYMENT/KEYPEDIA) | ❌ Tidak ada (hanya DIGI client) |
| 14 | Error log viewer + cronsjob reference | ❌ Tidak ada |
| 15 | Manual-order center + resend WA | ❌ Tidak ada |
