/* ============================================================
   Aruteru Shoppu — Shared Alpine data + utilities for CSR pages
   Load AFTER Alpine.js, BEFORE page-specific scripts.
   ============================================================ */

function tryDecodeBase64(str) {
    if (!str) return '';
    try {
        return atob(str);
    } catch (e) {
        return str;
    }
}

function slugify(s) {
    return String(s || '').toLowerCase().replace(/\s+/g, '-');
}

async function fetchJSON(url, options) {
    const res = await fetch(url, options);
    if (!res.ok) throw new Error('HTTP ' + res.status);
    return res.json();
}

/* Format angka jadi rupiah: 20000 -> "20.000" */
function fmt(n) {
    if (n === null || n === undefined || isNaN(n)) return '0';
    return Number(n).toLocaleString('id-ID');
}

/* Copy teks ke clipboard (navigator API + fallback) */
function copyToClipboard(text) {
    if (navigator.clipboard && window.isSecureContext) {
        return navigator.clipboard.writeText(text);
    }
    return new Promise((resolve) => {
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand('copy'); } catch (e) {}
        document.body.removeChild(ta);
        resolve();
    });
}

/* Masking order_id: ORD20260807123456 -> ORD2**********3456 */
function hideString(s) {
    s = String(s || '');
    if (s.length <= 6) return s.replace(/./g, '*');
    return s.slice(0, 3) + '*'.repeat(Math.max(4, s.length - 7)) + s.slice(-4);
}

/* Toast notif (pakai #react-notif yang di-inject layout.js) */
function showToast(message, type = 'error') {
    const container = document.getElementById('react-notif');
    if (!container) { alert(message); return; }
    const toast = document.createElement('div');
    toast.className = 'toast ' + (type === 'success' ? 'success' : '');
    const icon = document.createElement('div');
    icon.className = 'toast-icon';
    icon.innerHTML = type === 'success'
        ? '<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" width="16" color="rgba(34,197,94,0.8)"><path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7"></path></svg>'
        : '<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" width="16" color="rgba(244,63,94,0.8)"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12"></path></svg>';
    const msg = document.createElement('div');
    msg.className = 'toast-message';
    msg.textContent = message;
    toast.appendChild(icon);
    toast.appendChild(msg);
    container.appendChild(toast);
    setTimeout(() => toast.remove(), 3000);
}

/* Apply warna konfigurasi (config.colors) ke CSS variables :root
   PHP memakai conf('color',1..4) -> --warna_1..4. CSR replikasi. */
function applyConfigColors(colors) {
    const root = document.documentElement;
    const c = Array.isArray(colors) ? colors : [];
    for (let i = 0; i < 4; i++) {
        if (c[i]) root.style.setProperty('--warna_' + (i + 1), c[i]);
    }
}

/* Shared layout state used by every CSR page */
function layoutData() {
    return {
        isSearchModalOpen: false,
        searchResults: [],
        searchLoading: false,
        searchTimer: null,
        hasSearched: false,
        config: {
            flashsale_time: '',
            colors: ['', '', '', ''],
            social: ['', '', '', '', ''],
            footer_enabled: true,
            ui_category: false
        },
        popup: { show: false, image: '', subtitle: '', content: '' },
        showPopup: false,
        loadConfig() {
            return fetchJSON('/api/v1/page-data')
                .then(data => {
                    if (data.config) this.config = Object.assign(this.config, data.config);
                    applyConfigColors(this.config.colors);
                    if (data.config && data.config.popup) {
                        this.popup = data.config.popup;
                        const hidden = localStorage.getItem('hidePopupp') === 'true';
                        if (this.popup.show && !hidden) {
                            this.$nextTick(() => { this.showPopup = true; });
                        }
                    }
                })
                .catch(err => console.error('Gagal memuat konfigurasi', err));
        },
        doSearch(query) {
            clearTimeout(this.searchTimer);
            if (!query || query.trim().length < 2) {
                this.searchResults = [];
                this.hasSearched = false;
                return;
            }
            this.searchLoading = true;
            this.hasSearched = true;
            this.searchTimer = setTimeout(() => {
                fetchJSON(`/api/v1/search?q=${encodeURIComponent(query.trim())}`)
                    .then(data => { this.searchResults = data.data || []; })
                    .catch(err => console.error('Search gagal', err))
                    .finally(() => { this.searchLoading = false; });
            }, 1000);
        },
        tryDecodeBase64
    };
}
