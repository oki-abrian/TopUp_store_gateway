//! ============================================================
//! ARUTERU SHOPPU — FULFILLMENT CLI DASHBOARD
//! ============================================================
//! Antarmuka terminal bergaya web untuk mengelola SERVER TOPUP
//! (fulfillment) langsung pada DB-nya sendiri.
//!
//! - TIDAK membuka port HTTP apa pun (aman, zero-inbound).
//! - TIDAK menyentuh database Server Web sama sekali.
//! - Sinkronisasi harga/promo ke web dilakukan via Bot 3 (Telegram
//!   terenkripsi), bisa dipicu manual dari menu [7].
//!
//! Jalankan:  cargo run --bin fulfillment_cli

use rust_backend::config::Config;
use rust_backend::domain::telegram::sync_bot;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::Row;

// ---------- ANSI helpers ----------

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";

fn rupiah(n: f64) -> String {
    let n = n as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    let bytes = s.as_bytes();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(*c as char);
    }
    if n < 0 { format!("-Rp{}", out) } else { format!("Rp{}", out) }
}

fn clear() {
    print!("\x1b[2J\x1b[H");
}

fn banner() {
    println!("{}{}╔══════════════════════════════════════════════════════════════╗", BOLD, CYAN);
    println!("║   ⚡ ARUTERU SHOPPU — FULFILLMENT CONTROL PANEL (SERVER TOPUP)   ║");
    println!("║   Zero-inbound • DB Topup Only • Sync via Telegram Bot 3       ║");
    println!("╚══════════════════════════════════════════════════════════════╝{}", RESET);
}

fn header(title: &str) {
    clear();
    banner();
    println!();
    println!("{}{}┌─ {} {}{}", BOLD, BLUE, title, " ".repeat(58usize.saturating_sub(title.len())), RESET);
    println!("{}│{}", BLUE, RESET);
}

fn footer_hint() {
    println!("{}│{}", BLUE, RESET);
    println!("{}└──────────────────────────────────────────────────────────────{}", BLUE, RESET);
    println!();
}

fn pause() {
    println!();
    println!("{}Tekan Enter untuk kembali ke menu...{}", DIM, RESET);
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}

fn ask(prompt: &str) -> String {
    print!("{}{} ▸ {}{}", GREEN, BOLD, prompt, RESET);
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).unwrap_or(0);
    buf.trim().to_string()
}

fn ok(msg: &str) {
    println!("{}✔ {}{}", GREEN, msg, RESET);
}
fn err(msg: &str) {
    println!("{}✘ {}{}", RED, msg, RESET);
}

fn table_row(cols: &[String], widths: &[usize]) {
    let mut line = String::from("  ");
    for (c, w) in cols.iter().zip(widths.iter()) {
        line.push_str(&format!("{:<width$} ", truncate(c, *w), width = *w));
    }
    println!("{}", line);
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let t: String = s.chars().take(w.saturating_sub(2)).collect();
        format!("{}…", t)
    }
}

// ---------- Screens ----------

async fn screen_dashboard(db: &sqlx::MySqlPool) {
    header("DASHBOARD");

    async fn scalar_i(db: &sqlx::MySqlPool, q: &str) -> i64 {
        sqlx::query_scalar::<sqlx::MySql, i64>(q).fetch_one(db).await.unwrap_or(0)
    }
    async fn scalar_f(db: &sqlx::MySqlPool, q: &str) -> f64 {
        sqlx::query_scalar::<sqlx::MySql, f64>(q).fetch_one(db).await.unwrap_or(0.0)
    }

    let trx_today = scalar_i(db, "SELECT COUNT(*) FROM transaction WHERE DATE(created_at) = CURDATE()").await;
    let success_today = scalar_i(db, "SELECT COUNT(*) FROM transaction WHERE status='success' AND DATE(created_at)=CURDATE()").await;
    let pending = scalar_i(db, "SELECT COUNT(*) FROM transaction WHERE status IN ('pending','process')").await;
    let revenue_today = scalar_f(db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status='success' AND DATE(created_at)=CURDATE()").await;
    let revenue_total = scalar_f(db, "SELECT COALESCE(SUM(price),0) FROM transaction WHERE status='success'").await;
    let users = scalar_i(db, "SELECT COUNT(*) FROM users").await;
    let balance_total = scalar_f(db, "SELECT COALESCE(SUM(balance),0) FROM users").await;
    let services = scalar_i(db, "SELECT COUNT(*) FROM service WHERE status='available'").await;

    println!("  {}Statistik Hari Ini{}", BOLD, RESET);
    println!("  ────────────────────────────────────────────────");
    println!("   Order Masuk     : {}{}{} transaksi", CYAN, trx_today, RESET);
    println!("   Sukses          : {}{}{}", GREEN, success_today, RESET);
    println!("   Pending/Proses  : {}{}{}", YELLOW, pending, RESET);
    println!("   Revenue Hari Ini: {}{}{}", GREEN, rupiah(revenue_today), RESET);
    println!();
    println!("  {}Akumulasi{}", BOLD, RESET);
    println!("  ────────────────────────────────────────────────");
    println!("   Revenue Total   : {}{}{}", GREEN, rupiah(revenue_total), RESET);
    println!("   Total User      : {}{}{}", CYAN, users, RESET);
    println!("   Saldo User      : {}{}{}", MAGENTA, rupiah(balance_total), RESET);
    println!("   Layanan Aktif   : {}{}{}", BLUE, services, RESET);

    // Mini bar chart order 7 hari
    println!();
    println!("  {}Order 7 Hari Terakhir{}", BOLD, RESET);
    let rows = sqlx::query("SELECT DATE(created_at) d, COUNT(*) c FROM transaction WHERE created_at >= DATE_SUB(CURDATE(), INTERVAL 7 DAY) GROUP BY d ORDER BY d")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    let max = rows.iter().map(|r| r.try_get::<i64, _>("c").unwrap_or(0)).max().unwrap_or(1).max(1);
    for r in rows {
        let d: chrono::NaiveDateTime = r.try_get("d").unwrap_or_default();
        let c: i64 = r.try_get("c").unwrap_or(0);
        let bars = ((c as f64 / max as f64) * 30.0).ceil() as usize;
        println!("   {} {}{}{} {} {}", d.format("%d/%m"), GREEN, "█".repeat(bars), RESET, DIM, RESET);
        print!("{}", DIM);
        println!("      {} order{}", c, RESET);
    }

    footer_hint();
    pause();
}

async fn screen_services(db: &sqlx::MySqlPool) {
    loop {
        header("KELOLA LAYANAN");
        println!("  1. Lihat / Cari Layanan");
        println!("  2. Ubah Harga & Margin");
        println!("  3. Ubah Status (available/empty)");
        println!("  0. Kembali");
        match ask("Pilih: ").as_str() {
            "1" => list_services(db).await,
            "2" => edit_service_price(db).await,
            "3" => edit_service_status(db).await,
            "0" | "" => break,
            _ => {}
        }
    }
}

async fn list_services(db: &sqlx::MySqlPool) {
    header("DAFTAR LAYANAN");
    let q = ask("Cari nama/kode (kosong = semua): ").to_lowercase();
    let pattern = format!("%{}%", q);
    let rows = sqlx::query("SELECT code, name, price, member, reseller, provider, status FROM service WHERE LOWER(name) LIKE ? OR LOWER(code) LIKE ? ORDER BY name LIMIT 40")
        .bind(&pattern).bind(&pattern)
        .fetch_all(db)
        .await
        .unwrap_or_default();

    let widths = [18usize, 34, 12, 10, 10, 8, 9];
    let headers: Vec<String> = vec![
        "KODE".into(), "NAMA".into(), "HARGA".into(), "MEMBER".into(),
        "RESELLER".into(), "PROV".into(), "STATUS".into(),
    ];
    table_row(&headers, &widths);
    println!("  {}", "-".repeat(105));
    for r in rows {
        let code: String = r.try_get("code").unwrap_or_default();
        let name: String = r.try_get("name").unwrap_or_default();
        let price: f64 = r.try_get("price").unwrap_or(0.0);
        let member: f64 = r.try_get("member").unwrap_or(0.0);
        let reseller: f64 = r.try_get("reseller").unwrap_or(0.0);
        let provider: String = r.try_get("provider").unwrap_or_default();
        let status: String = r.try_get("status").unwrap_or_default();
        let status_colored = if status == "available" {
            format!("{}{}{}", GREEN, status, RESET)
        } else {
            format!("{}{}{}", RED, status, RESET)
        };
        table_row(
            &[code, name, rupiah(price), rupiah(member), rupiah(reseller), provider, status_colored]
                .map(String::from),
            &widths,
        );
    }
    footer_hint();
    pause();
}

async fn pick_service(db: &sqlx::MySqlPool) -> Option<String> {
    let code = ask("Kode layanan: ");
    if code.is_empty() {
        return None;
    }
    match sqlx::query("SELECT code, name, price, member, reseller, status FROM service WHERE code = ?")
        .bind(&code)
        .fetch_optional(db)
        .await
    {
        Ok(Some(r)) => {
            let name: String = r.try_get("name").unwrap_or_default();
            let price: f64 = r.try_get("price").unwrap_or(0.0);
            let member: f64 = r.try_get("member").unwrap_or(0.0);
            let reseller: f64 = r.try_get("reseller").unwrap_or(0.0);
            let status: String = r.try_get("status").unwrap_or_default();
            println!("   {}{}{} — harga {}, member +{}, reseller +{}, status {}", BOLD, name, RESET, rupiah(price), rupiah(member), rupiah(reseller), status);            Some(code)
        }
        _ => {
            err("Layanan tidak ditemukan.");
            None
        }
    }
}

async fn edit_service_price(db: &sqlx::MySqlPool) {
    header("UBAH HARGA LAYANAN");
    if let Some(code) = pick_service(db).await {
        let p = ask("Harga dasar baru: ").parse::<f64>().ok();
        let m = ask("Margin member baru (Enter=skip): ").parse::<f64>().ok();
        let r = ask("Margin reseller baru (Enter=skip): ").parse::<f64>().ok();
        if let Some(price) = p {
            let res = sqlx::query("UPDATE service SET price = ?, member = COALESCE(?, member), reseller = COALESCE(?, reseller), date_up = NOW() WHERE code = ?")
                .bind(price).bind(m).bind(r).bind(&code)
                .execute(db)
                .await;
            match res {
                Ok(_) => ok(&format!("Harga {} diperbarui → {}. Jangan lupa [7] Sync ke Web.", code, rupiah(price))),
                Err(e) => err(&format!("Gagal: {}", e)),
            }
        }
    }
    footer_hint();
    pause();
}

async fn edit_service_status(db: &sqlx::MySqlPool) {
    header("UBAH STATUS LAYANAN");
    if let Some(code) = pick_service(db).await {
        let s = ask("Status baru (available/empty): ");
        if s == "available" || s == "empty" {
            let res = sqlx::query("UPDATE service SET status = ?, date_up = NOW() WHERE code = ?")
                .bind(&s)
                .bind(&code)
                .execute(db)
                .await;
            match res {
                Ok(_) => ok("Status diperbarui."),
                Err(e) => err(&format!("Gagal: {}", e)),
            }
        } else {
            err("Status harus 'available' atau 'empty'.");
        }
    }
    footer_hint();
    pause();
}

async fn screen_flashsale(db: &sqlx::MySqlPool) {
    loop {
        header("KELOLA FLASHSALE");
        println!("  1. Lihat Flashsale");
        println!("  2. Tambah Flashsale");
        println!("  3. Aktifkan / Matikan");
        println!("  4. Hapus");
        println!("  0. Kembali");
        match ask("Pilih: ").as_str() {
            "1" => {
                header("DAFTAR FLASHSALE");
                let rows = sqlx::query("SELECT f.id, f.code, f.amount, f.status, COALESCE(s.name,'') sn FROM flashsale f LEFT JOIN service s ON s.code=f.code ORDER BY f.id DESC")
                    .fetch_all(db).await.unwrap_or_default();
                for r in rows {
                    let id: i64 = r.try_get("id").unwrap_or(0);
                    let code: String = r.try_get("code").unwrap_or_default();
                    let sn: String = r.try_get("sn").unwrap_or_default();
                    let amount: f64 = r.try_get("amount").unwrap_or(0.0);
                    let status: String = r.try_get("status").unwrap_or_default();
                    let st = if status == "1" { format!("{}AKTIF{}", GREEN, RESET) } else { format!("{}MATI{}", DIM, RESET) };
                    println!("   #{} {} ({}) − {} [{}]", id, code, sn, rupiah(amount), st);
                }
                footer_hint();
                pause();
            }
            "2" => {
                let code = ask("Kode layanan: ");
                let amount = ask("Diskon (Rp): ").parse::<f64>().ok();
                if !code.is_empty() {
                    if let Some(amount) = amount {
                        match sqlx::query("INSERT INTO flashsale (code, amount, status) VALUES (?, ?, '1')").bind(&code).bind(amount).execute(db).await {
                            Ok(_) => ok("Flashsale ditambahkan (aktif)."),
                            Err(e) => err(&format!("Gagal: {}", e)),
                        }
                    }
                }
                pause();
            }
            "3" => {
                let id = ask("ID flashsale: ").parse::<i64>().ok();
                if let Some(id) = id {
                    match sqlx::query("UPDATE flashsale SET status = IF(status='1','0','1') WHERE id = ?").bind(id).execute(db).await {
                        Ok(_) => ok("Status diubah."),
                        Err(e) => err(&format!("Gagal: {}", e)),
                    }
                }
                pause();
            }
            "4" => {
                let id = ask("ID flashsale: ").parse::<i64>().ok();
                if let Some(id) = id {
                    let _ = sqlx::query("DELETE FROM flashsale WHERE id = ?").bind(id).execute(db).await;
                    ok("Dihapus.");
                }
                pause();
            }
            "0" | "" => break,
            _ => {}
        }
    }
}

async fn screen_voucher(db: &sqlx::MySqlPool) {
    loop {
        header("KELOLA VOUCHER");
        println!("  1. Lihat Voucher");
        println!("  2. Tambah Voucher");
        println!("  3. Hapus Voucher");
        println!("  0. Kembali");
        match ask("Pilih: ").as_str() {
            "1" => {
                header("DAFTAR VOUCHER");
                let rows = sqlx::query("SELECT voucher, code, discount, minimum, stock FROM voucher ORDER BY id DESC LIMIT 50")
                    .fetch_all(db).await.unwrap_or_default();
                if rows.is_empty() {
                    println!("   (belum ada voucher)");
                }
                for r in rows {
                    let v: String = r.try_get("voucher").unwrap_or_default();
                    let cat: String = r.try_get("code").unwrap_or_default();
                    let disc: f64 = r.try_get("discount").unwrap_or(0.0);
                    let min: f64 = r.try_get("minimum").unwrap_or(0.0);
                    let stock: i64 = r.try_get("stock").unwrap_or(0);
                    let sc = if stock > 0 { format!("{}{}{}", GREEN, stock, RESET) } else { format!("{}0{}", RED, RESET) };
                    println!("   {}{}{} │ kategori: {} │ diskon {} │ min {} │ stok {}", BOLD, v, RESET, if cat.is_empty() { "SEMUA" } else { &cat }, rupiah(disc), rupiah(min), sc);
                }
                footer_hint();
                pause();
            }
            "2" => {
                let v = ask("Kode voucher: ");
                let cat_raw = ask("Kategori (Enter=semua): ");
                let disc = ask("Diskon (Rp): ").parse::<f64>().ok();
                let min = ask("Minimum belanja (Enter=0): ").parse::<f64>().ok();
                let stock = ask("Stok: ").parse::<i64>().ok();
                if !v.is_empty() {
                    if let (Some(disc), Some(stock)) = (disc, stock) {
                        let cat = if cat_raw.is_empty() { String::new() } else { cat_raw };
                        match sqlx::query("INSERT INTO voucher (code, voucher, discount, minimum, stock, date_cr) VALUES (?, ?, ?, ?, ?, NOW())")
                            .bind(&cat).bind(&v).bind(disc).bind(min.unwrap_or(0.0)).bind(stock)
                            .execute(db).await
                        {
                            Ok(_) => ok("Voucher dibuat."),
                            Err(e) => err(&format!("Gagal (tabel voucher ada?): {}", e)),
                        }
                    }
                }
                pause();
            }
            "3" => {
                let v = ask("Kode voucher yang dihapus: ");
                let _ = sqlx::query("DELETE FROM voucher WHERE voucher = ?").bind(&v).execute(db).await;
                ok("Selesai.");
                pause();
            }
            "0" | "" => break,
            _ => {}
        }
    }
}

async fn screen_transactions(db: &sqlx::MySqlPool) {
    header("TRANSAKSI TERBARU");
    let rows = sqlx::query("SELECT order_id, user, code, service_name, target, price, status, payment_status, created_at FROM transaction ORDER BY id DESC LIMIT 25")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    for r in rows {
        let oid: String = r.try_get("order_id").unwrap_or_default();
        let user: String = r.try_get("user").unwrap_or_default();
        let svc: String = r.try_get("service_name").unwrap_or_default();
        let price: f64 = r.try_get("price").unwrap_or(0.0);
        let status: String = r.try_get("status").unwrap_or_default();
        let pay: String = r.try_get("payment_status").unwrap_or_default();
        let created: Option<chrono::NaiveDateTime> = r.try_get("created_at").unwrap_or(None);
        let sc = match status.as_str() {
            "success" => format!("{}{}{}", GREEN, status, RESET),
            "error" | "system" => format!("{}{}{}", RED, status, RESET),
            _ => format!("{}{}{}", YELLOW, status, RESET),
        };
        let pc = if pay == "paid" { format!("{}{}{}", GREEN, pay, RESET) } else { format!("{}{}{}", YELLOW, pay, RESET) };
        println!(
            "  {} {} │ {} │ {} │ {} {}",
            DIM,
            created.map(|d| d.format("%d/%m %H:%M").to_string()).unwrap_or_default(),
            RESET, oid, user, truncate(&svc, 28),
        );
        println!("         {} │ target: {} │ {} {}", sc, truncate(&r.try_get::<String, _>("target").unwrap_or_default(), 24), rupiah(price), pc);
    }
    footer_hint();
    pause();
}

async fn screen_users(db: &sqlx::MySqlPool) {
    header("USER & SALDO");
    let rows = sqlx::query("SELECT username, phone, balance, level FROM users ORDER BY balance DESC LIMIT 30")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    for r in rows {
        let u: String = r.try_get("username").unwrap_or_default();
        let phone: String = r.try_get("phone").unwrap_or_default();
        let bal: f64 = r.try_get("balance").unwrap_or(0.0);
        let lvl: String = r.try_get("level").unwrap_or_default();
        println!("   {:<16} {:<15} {:>14}  {}", u, phone, rupiah(bal), lvl);
    }
    footer_hint();
    pause();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    let db = MySqlPoolOptions::new()
        .max_connections(5)
        .connect_lazy(&config.database_url)?;

    loop {
        clear();
        banner();
        println!();
        println!("  {}MENU UTAMA{}", BOLD, RESET);
        println!("  ────────────────────────────────────────────────");
        println!("   {}1{}. 📊  Dashboard", BOLD, RESET);
        println!("   {}2{}. 🎮  Kelola Layanan (harga/margin/status)", BOLD, RESET);
        println!("   {}3{}. 🔥  Kelola Flashsale", BOLD, RESET);
        println!("   {}4{}. 🎟️  Kelola Voucher/Promo", BOLD, RESET);
        println!("   {}5{}. 🧾  Transaksi Terbaru", BOLD, RESET);
        println!("   {}6{}. 👤  User & Saldo", BOLD, RESET);
        println!("   {}7{}. 🔄  SYNC harga+promo ke Server Web (Bot 3)", BOLD, RESET);
        println!("   {}0{}. 🚪  Keluar", BOLD, RESET);
        println!();

        match ask("Pilih menu: ").as_str() {
            "1" => screen_dashboard(&db).await,
            "2" => screen_services(&db).await,
            "3" => screen_flashsale(&db).await,
            "4" => screen_voucher(&db).await,
            "5" => screen_transactions(&db).await,
            "6" => screen_users(&db).await,
            "7" => {
                header("SYNC KE SERVER WEB");
                println!("  Mendorong snapshot layanan + flashsale + voucher...");
                match sync_bot::sync_now(&db).await {
                    Ok(n) => ok(&format!("{} perintah sync terkirim via Telegram terenkripsi.", n)),
                    Err(e) => err(&format!("Sync gagal: {}", e)),
                }
                footer_hint();
                pause();
            }
            "0" | "" => {
                clear();
                println!("Sampai jumpa! 👋");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
