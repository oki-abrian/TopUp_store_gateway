/* ============================================================
   Aruteru Shoppu — Shared CSR Layout
   Injects navbar, footer, search modal, popup, theme toggle,
   and CS chat into every static CSR page.
   Requires Alpine.js 3.x loaded BEFORE this file.
   ============================================================ */

(function () {
    function el(html) {
        const t = document.createElement('template');
        t.innerHTML = html.trim();
        return t.content.firstElementChild;
    }

    function formatRupiah(n) {
        if (n === null || n === undefined || isNaN(n)) return '0';
        return Number(n).toLocaleString('id-ID');
    }

    const NAV_ICONS = {
        home: '<svg class="nav-icon text-white" viewBox="0 0 24 24"><path d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6"></path></svg>',
        invoices: '<svg class="nav-icon text-white" viewBox="0 0 24 24"><path d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4"></path></svg>',
        region: '<svg class="nav-icon text-white" viewBox="0 0 24 24"><path d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z"></path></svg>',
        price: '<svg class="nav-icon text-white" viewBox="0 0 24 24"><path d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A1.994 1.994 0 013 12V7a4 4 0 014-4z"></path></svg>',
        leaderboard: '<svg class="nav-icon text-white" viewBox="0 0 24 24"><path d="M9 12l2 2 4-4M7.835 4.697a3.42 3.42 0 001.946-.806 3.42 3.42 0 014.438 0 3.42 3.42 0 001.946.806 3.42 3.42 0 013.138 3.138 3.42 3.42 0 00.806 1.946 3.42 3.42 0 010 4.438 3.42 3.42 0 00-.806 1.946 3.42 3.42 0 01-3.138 3.138 3.42 3.42 0 00-1.946.806 3.42 3.42 0 01-4.438 0 3.42 3.42 0 00-1.946-.806 3.42 3.42 0 01-3.138-3.138 3.42 3.42 0 00-.806-1.946 3.42 3.42 0 010-4.438 3.42 3.42 0 00.806-1.946 3.42 3.42 0 013.138-3.138z"></path></svg>',
        login: '<i class="bi bi-box-arrow-in-right"></i>'
    };

    const ICONLY = {
        home: '<div style="--size:14px;" class="relative size-[--size] inline-block"><div style="--size:14px;--icon-color:hsl(var(--primary))" class="iconly [&>svg]:!size-[--size]"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" style="width: 100%; height: 100%;"><g transform="matrix(1,0,0,1,12,12)" style="display: block;"><path class="main-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="1" stroke-width="2" d="M-6 -2L0 -8L6 -2"/><path class="secondary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.6" stroke-width="1.5" d="M-6 -1V8a2 2 0 002 2h3"/><path class="tertiary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.4" stroke-width="1.5" d="M6 -1V8a2 2 0 01-2 2h-3 M-4 6V8a2 2 0 002 2h4a2 2 0 002-2V6"/></g></svg></div></div>',
        invoices: '<div style="--size:14px;" class="relative size-[--size] inline-block"><div style="--size:14px;--icon-color:hsl(var(--primary))" class="iconly [&>svg]:!size-[--size]"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" style="width: 100%; height: 100%;"><g transform="matrix(1,0,0,1,12,12)" style="display: block;"><path class="main-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="1" stroke-width="2" d="M-4 -8h-3a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V-6a2 2 0 00-2-2h-3"/><path class="secondary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.6" stroke-width="1.5" d="M-4 -8a2 2 0 012 2h2a2 2 0 012-2M-4 -8a2 2 0 012-2h2a2 2 0 012 2"/><path class="tertiary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.4" stroke-width="1.5" d="M-6 1l2 2 4-4"/></g></svg></div></div>',
        region: '<div style="--size:14px;" class="relative size-[--size] inline-block"><div style="--size:14px;--icon-color:hsl(var(--primary))" class="iconly [&>svg]:!size-[--size]"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" style="width: 100%; height: 100%;"><g transform="matrix(1,0,0,1,12,12)" style="display: block;"><path class="main-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="1" stroke-width="2" d="M-7 -1h2a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v3"/><path class="secondary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.6" stroke-width="1.5" d="M-4 -8v1.5a2.5 2.5 0 002.5 2.5h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1"/><path class="tertiary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.4" stroke-width="1.5" d="M3 8v-2a2 2 0 012-2h3 M9 0a9 9 0 11-18 0 9 9 0 0118 0z"/></g></svg></div></div>',
        price: '<div style="--size:14px;" class="relative size-[--size] inline-block"><div style="--size:14px;--icon-color:hsl(var(--primary))" class="iconly [&>svg]:!size-[--size]"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" style="width: 100%; height: 100%;"><g transform="matrix(1,0,0,1,12,12)" style="display: block;"><path class="main-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="1" stroke-width="2" d="M-4 -4h.01M-4 -8h4c.51 0 1.02.2 1.41.59l6 6a2 2 0 010 2.82l-6 6a2 2 0 01-2.82 0l-6-6A1.99 1.99 0 01-8 0v-4a4 4 0 014-4z"/></g></svg></div></div>',
        leaderboard: '<div style="--size:14px;" class="relative size-[--size] inline-block"><div style="--size:14px;--icon-color:hsl(var(--primary))" class="iconly [&>svg]:!size-[--size]"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" style="width: 100%; height: 100%;"><g transform="matrix(1,0,0,1,12,12)" style="display: block;"><path class="main-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="1" stroke-width="2" d="M0 -9V-6M-6 4h12M-4 -6h8v4h-8z"></path><path class="secondary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.6" stroke-width="1.5" d="M-2 -2v6M2 -2v6M-2 0h4"></path><path class="tertiary-path" stroke-linecap="round" stroke-linejoin="round" fill-opacity="0" stroke="rgb(255,255,255)" stroke-opacity="0.4" stroke-width="1.5" d="M-6 4l1 5h10l1-5"></path></g></svg></div></div>'
    };

    const navbarHTML = `
    <nav class="navbar sticky top-0 z-40 w-full flex-none blurred-navbar bg-murky-900 print:hidden">
        <div class="container">
            <div class="flex justify-between h-[60px] gap-2">
                <div class="flex items-center md:hidden">
                    <button type="button" class="p-2 rounded-lg hover:bg-murky-700 focus:outline-none" aria-label="Toggle menu" onclick="document.getElementById('mobile-menu').style.display='block'">
                        <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-6 h-6 text-white"><path stroke-linecap="round" stroke-linejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" /></svg>
                    </button>
                </div>
                <div class="flex items-center md:w-[160px]">
                    <a style="outline: none;" href="/"><img src="/imagehome/Footeraru.png" fetchpriority="high" width="150" height="150" decoding="async" class="h-9 w-auto lg:h-10" style="color:transparent" onerror="this.outerHTML='<span class=\\'text-xl font-bold text-white\\'>Aruteru Shoppu</span>'"></a>
                </div>
                <div class="md:hidden flex items-center gap-2 ml-auto">
                    <button @click="isSearchModalOpen = true" type="button" class="p-2 rounded-lg hover:bg-murky-700 focus:outline-none" aria-label="Cari Game atau Voucher">
                        <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-6 h-6 text-white"><path stroke-linecap="round" stroke-linejoin="round" d="M21 21l-5.197-5.197m0 0A7.5 7.5 0 105.196 5.196a7.5 7.5 0 0010.607 10.607z"></path></svg>
                    </button>
                </div>
                <div class="hidden md:flex flex-1 max-w-4xl px-6 py-2">
                    <div class="relative w-full my-1">
                        <input type="text" x-on:click="isSearchModalOpen = true" class="search-input h-10 rounded-full pl-11 pr-4 text-white placeholder-gray-400 focus:outline-none w-full text-[15px]" placeholder="Cari Game atau Voucher" readonly>
                        <div class="absolute inset-y-0 left-4 flex items-center pointer-events-none">
                            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="h-5 w-5 text-gray-400"><path stroke-linecap="round" stroke-linejoin="round" d="M21 21l-5.197-5.197m0 0A7.5 7.5 0 105.196 5.196a7.5 7.5 0 0010.607 10.607z"></path></svg>
                        </div>
                    </div>
                </div>
                <div class="md:w-[160px]"></div>
            </div>
            <div class="mobile-menu-container md:hidden bg-murky-900 border-t border-gray-700 shadow-lg" id="mobile-menu" style="display: none;">
                <div class="mobile-menu-content">
                    <a href="/" class="mobile-nav-link"><div class="flex items-center">${NAV_ICONS.home}<span class="ml-2">Beranda</span></div></a>
                    <a href="/invoices" class="mobile-nav-link"><div class="flex items-center">${NAV_ICONS.invoices}<span class="ml-2">Cek Transaksi</span></div></a>
                    <a href="/page/region" class="mobile-nav-link"><div class="flex items-center">${NAV_ICONS.region}<span class="ml-2">Cek Region</span></div></a>
                    <a href="/page/pricelist" class="mobile-nav-link"><div class="flex items-center">${NAV_ICONS.price}<span class="ml-2">Harga</span></div></a>
                    <a href="/app/guest/leaderboard" class="mobile-nav-link"><div class="flex items-center">${NAV_ICONS.leaderboard}<span class="ml-2">Leaderboard</span></div></a>
                    <a href="/auth/login" class="mobile-nav-link" id="mobile-login-link"><div class="flex items-center"><i class="bi bi-box-arrow-in-right"></i><span class="ml-2">Masuk</span></div></a>
                    <div class="pt-4 flex items-center justify-between">
                        <button id="close-menu-btn" onclick="document.getElementById('mobile-menu').style.display='none'" class="close-menu-button bg-gray-800 text-white p-3 rounded-lg hover:bg-gray-700 transition-all duration-300 flex items-center gap-2">
                            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2" stroke="currentColor" class="w-5 h-5"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                            <span>Tutup Menu</span>
                        </button>
                    </div>
                </div>
            </div>
            <div class="hidden lg:block border-t border-gray-700">
                <div class="flex items-center justify-between py-0">
                    <div class="flex space-x-4 pl-0 md:pl-[40px]">
                        <a href="/" class="desktop-nav-link" style="outline: none;">${ICONLY.home}<span class="ml-2">Beranda</span></a>
                        <a href="/invoices" class="desktop-nav-link" style="outline: none;">${ICONLY.invoices}<span class="ml-2">Cek Transaksi</span></a>
                        <a href="/page/region" class="desktop-nav-link" style="outline: none;">${ICONLY.region}<span class="ml-2">Cek Region</span></a>
                        <a href="/page/pricelist" class="desktop-nav-link" style="outline: none;">${ICONLY.price}<span class="ml-2">Harga</span></a>
                        <a href="/app/guest/leaderboard" class="desktop-nav-link" style="outline: none;">${ICONLY.leaderboard}<span class="ml-2">Leaderboard</span></a>
                    </div>
                    <div class="flex items-center gap-4">
                        <button id="theme-toggle" class="text-sm font-medium text-white bg-melpa-100 from-murky-800 to-murky-800 border rounded-xl px-3 py-2 hover:bg-murky-700 transition-colors duration-200" style="outline: none;">
                            <i class="bi bi-sun-fill dark-icon"></i>
                            <i class="bi bi-moon-fill light-icon" style="display: none;"></i>
                        </button>
                        <a href="/auth/login" class="text-sm font-medium text-white bg-melpa-100 from-murky-800 to-murky-800 border rounded-xl px-4 py-2 hover:bg-murky-700 transition-colors duration-200" style="outline: none;" id="desktop-login-link">
                            <i class="bi bi-box-arrow-in-right mr-2"></i><span>Masuk</span>
                        </a>
                    </div>
                </div>
            </div>
        </div>
    </nav>`;

    const searchModalHTML = `
    <div class="relative z-50" role="dialog" tabindex="-1" x-show="isSearchModalOpen" x-on:click.away="isSearchModalOpen = false" x-cloak x-transition>
        <div class="fixed inset-0 z-50 overflow-hidden p-4 py-20 sm:py-20 sm:px-6 md:p-20">
            <div class="fixed inset-0 bg-gray-500 bg-opacity-25 transition-opacity opacity-100" x-show="isSearchModalOpen" x-cloak x-on:click="isSearchModalOpen=false"></div>
            <div class="mx-auto max-w-2xl transform divide-y divide-gray-500 divide-opacity-10 overflow-hidden rounded-xl bg-murky-700 bg-opacity-80 shadow-2xl ring-1 ring-black ring-opacity-5 backdrop-blur backdrop-filter transition-all opacity-100 scale-100">
                <div class="relative">
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="pointer-events-none absolute left-4 top-3.5 h-5 w-5 text-white text-opacity-40"><path fill-rule="evenodd" d="M9 3.5a5.5 5.5 0 100 11 5.5 5.5 0 000-11zM2 9a7 7 0 1112.452 4.391l3.328 3.329a.75.75 0 11-1.06 1.06l-3.329-3.328A7 7 0 012 9z" clip-rule="evenodd"></path></svg>
                    <form>
                        <input class="h-12 w-full border-0 bg-transparent pl-11 pr-4 text-white focus:ring-0 sm:text-sm" placeholder="Cari Game atau Voucher" id="searchProds" role="combobox" type="text" name="q" aria-expanded="false" aria-autocomplete="list" value="" tabindex="0" x-on:input="doSearch($event.target.value)">
                    </form>
                </div>
                <ul class="resultsearch max-h-80 scroll-py-2 divide-y divide-gray-500 divide-opacity-10 overflow-y-auto">
                    <div class="flex flex-col gap-2 items-center justify-center py-5" id="lottie-container" x-show="searchResults.length === 0 && !searchLoading">
                        <lottie-player src="https://lottie.host/29ee18a6-ecb8-4eb5-b94f-727a9043c858/ggYzBWGQJU.json" background="##fff" speed="1" style="width: 100px; height: 100px" loop autoplay direction="1" mode="normal"></lottie-player>
                        <span class="text-base text-center opacity-70 py-4">Ketik untuk mencari produk</span>
                    </div>
                    <template x-for="item in searchResults" :key="item.id">
                        <li class="p-2 dropdown-item animate__animated animate__fadeIn" style="animation-duration: 0.2s;">
                            <a :href="\`/product/\${item.type}/category/\${item.slug}\`" class="text-white">
                                <div class="flex cursor-pointer select-none items-center rounded-md px-3 py-2 hover:bg-gray-700 transition-all duration-200" role="option" tabindex="-1" aria-selected="false">
                                    <img :alt="item.name" loading="lazy" class="aspect-square w-24 rounded-2x1 object-cover" style="border-radius:10px" :src="item.image">
                                    <span class="ml-3 flex-autotruncate" x-text="item.name"></span>
                                </div>
                            </a>
                        </li>
                    </template>
                    <div class="flex flex-col gap-2 items-center justify-center py-5" x-show="searchResults.length === 0 && searchLoading"><span class="text-base text-center opacity-70 py-4">Mencari...</span></div>
                    <div class="flex flex-col gap-2 items-center justify-center py-5" x-show="searchResults.length === 0 && !searchLoading && hasSearched"><span class="text-base text-center opacity-70 py-4">Tidak ada hasil yang ditemukan</span></div>
                </ul>
            </div>
        </div>
    </div>`;

    const popupHTML = `
    <div class="popup-structuree popup-slidee flex min-h-full items-center justify-center p-4 text-center sm:p-0" id="popupp" x-show="showPopup" x-cloak>
        <div class="fixed inset-0 z-10 overflow-y-auto">
            <div class="flex min-h-full items-center justify-center p-4 text-center sm:p-0">
                <div class="relative w-full transform overflow-hidden rounded-lg bg-murky-900 text-left shadow-xl transition-all sm:my-8 sm:max-w-3xl !rounded-2xl opacity-100 translate-y-0 sm:scale-100" id="headlessui-dialog-panel-:rd:" data-headlessui-state="open">
                    <div class="absolute right-0 top-0 block pr-4 pt-4">
                        <button type="button" name="popupp" class="close-popup-buttonn rounded-md bg-murky-700 text-gray-400 hover:text-gray-500 focus:outline-none focus:ring-2 focus:ring-primary-500 focus:ring-offset-2 focus:ring-offset-murky-800" data-popup-id="popup-0">
                            <span class="sr-only">Close</span>
                            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" aria-hidden="true" class="h-6 w-6"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12"></path></svg>
                        </button>
                    </div>
                    <div class="w-full pb-4 flex flex-col items-center justify-start">
                        <div class="object-center"><img :src="popup.image" class="object-center" style="color: transparent; width: 100%; height: auto;" alt="" /></div>
                        <div class="flex w-full flex-col items-center justify-start pt-4 text-white">
                            <div class="flex w-full flex-col items-center px-4">
                                <div class="col-span-2 mt-1 flex w-full !text-xxs"><h2 class="max-w-xl pt-1 text-sm text-left font-semibold" x-text="popup.subtitle"></h2></div>
                            </div>
                            <div class="w-full max-w-none prose prose-sm px-4 py-4 text-xs items-center justify-start text-white" style="margin-top:-25px"><p x-text="tryDecodeBase64(popup.content)"></p></div>
                        </div>
                        <div class="flex w-full items-center justify-start px-4 pb-2">
                            <div class="flex items-center">
                                <input type="checkbox" id="dontshowinfoo" class="h-4 w-4 cursor-pointer rounded bg-murky-700 text-primary-500" />
                                <label class="block text-xs font-medium text-white ml-3 block select-none text-sm text-white !text-xxs !ml-2">Jangan tampilkan lagi</label>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </div>`;

    const footerHTML = `
    <div x-show="config.footer_enabled" x-cloak class="w-full overflow-hidden leading-none">
        <img src="/imagehome/Footeraru.png" alt="Footer Banner" width="700" height="475" style="width:100%;height:auto" class="object-cover object-bottom block">
    </div>
    <footer class="footer-wrapper">
        <div class="footer-container">
            <div class="footer-header">
                <div class="logo-container">
                    <span class="logo">
                        <img src="/imagehome/assets/Salinan dari NEW LOGO ARUTERU SHOPPU.png" fetchpriority="high" width="150" height="150" decoding="async" data-nimg="1" class="logo-image" style="color:transparent; max-height:50px; width:auto; margin:0 auto;" alt="Aruteru Shoppu" onerror="this.outerHTML='<span class=\\'text-2xl font-bold text-white\\'>Aruteru Shoppu</span>'">
                    </span>
                </div>
                <p class="description">Platform Top-Up Termurah dan Terpercaya.</p>
            </div>
            <div class="footer-grid">
                <div class="footer-section">
                    <div class="section-header"><h3 class="section-title"><i class="fas fa-map fa-fw"></i> Peta Situs</h3><div class="section-divider"></div></div>
                    <ul class="footer-links">
                        <li><a href="/invoices" class="footer-link"><i class="fas fa-receipt fa-fw"></i> Cek Transaksi</a></li>
                        <li><a :href="config.social[0] || '/contact'" class="footer-link"><i class="fas fa-headset fa-fw"></i> Hubungi Kami</a></li>
                    </ul>
                </div>
                <div class="footer-section">
                    <div class="section-header"><h3 class="section-title"><i class="fas fa-gavel fa-fw"></i> Legalitas</h3><div class="section-divider"></div></div>
                    <ul class="footer-links">
                        <li><a href="/page/privacy-policy" class="footer-link"><i class="fas fa-user-shield fa-fw"></i> Kebijakan Pribadi</a></li>
                        <li><a href="/page/terms-and-condition" class="footer-link"><i class="fas fa-file-contract fa-fw"></i> Syarat & Ketentuan</a></li>
                    </ul>
                </div>
                <div class="footer-section">
                    <div class="section-header"><h3 class="section-title"><i class="fas fa-hands-helping fa-fw"></i> Dukungan</h3><div class="section-divider"></div></div>
                    <ul class="footer-links">
                        <li><a :href="config.social[0]" class="footer-link" target="_blank" x-show="config.social[0]"><i class="fab fa-whatsapp fa-fw"></i> WhatsApp</a></li>
                        <li><a :href="config.social[1]" class="footer-link" target="_blank" x-show="config.social[1]"><i class="fab fa-instagram fa-fw"></i> Instagram</a></li>
                        <li><a :href="config.social[2]" class="footer-link" target="_blank" x-show="config.social[2]"><i class="fab fa-tiktok fa-fw"></i> Tiktok</a></li>
                        <li><a :href="config.social[3]" class="footer-link" target="_blank" x-show="config.social[3]"><i class="fab fa-facebook fa-fw"></i> Facebook</a></li>
                        <li><a :href="\`mailto:\${config.social[4]}?subject=Contact\`" class="footer-link" x-show="config.social[4]"><i class="fas fa-envelope fa-fw"></i> Email</a></li>
                    </ul>
                </div>
                <div class="footer-section">
                    <div class="section-header"><h3 class="section-title"><i class="fas fa-user-lock fa-fw"></i> Autentikasi</h3><div class="section-divider"></div></div>
                    <ul class="footer-links">
                        <li><a href="/auth/login" class="footer-link"><i class="fas fa-sign-in-alt fa-fw"></i> Masuk</a></li>
                        <li><a href="/auth/register" class="footer-link"><i class="fas fa-user-plus fa-fw"></i> Daftar</a></li>
                    </ul>
                </div>
            </div>
            <div class="footer-bottom">
                <div class="address-section">
                    <h3 class="address-title"><i class="fas fa-map-marker-alt fa-fw"></i> Alamat Store</h3>
                    <p class="address-text">Bojonggede, Kec. Waringin Jaya, Kabupaten Bogor, Jawa Barat</p>
                </div>
                <div class="copyright">© <span id="year"></span> Aruteru Shoppu. All rights reserved.</div>
            </div>
        </div>
    </footer>
    <div class="fixed bottom-0 left-4 z-50">
        <div id="cs-options" class="hidden mb-2 rounded-lg p-3 transform transition-all duration-300" style="background: rgba(255, 255, 255, 0.2); backdrop-filter: blur(15px); -webkit-backdrop-filter: blur(15px);">
            <div class="flex flex-col space-y-2">
                <a :href="config.social[0]" class="flex items-center space-x-2 p-2 hover:bg-white/30 rounded-lg transition-colors" target="_blank" rel="noopener noreferrer" x-show="config.social[0]"><i class="fa fa-whatsapp text-green-500 text-xl"></i><span class="text-white font-medium">WhatsApp</span></a>
                <a :href="\`mailto:\${config.social[4]}?subject=Bantuan&body=Halo, saya membutuhkan bantuan...\`" class="flex items-center space-x-2 p-2 hover:bg-white/30 rounded-lg transition-colors" target="_blank" rel="noopener noreferrer" x-show="config.social[4]"><i class="fa fa-envelope text-blue-500 text-xl"></i><span class="text-white font-medium">Email</span></a>
            </div>
        </div>
        <button onclick="toggleCsOptions()" class="inline-flex items-center space-x-2.5 duration-300 ease-in-out hover:animate-bounce focus:outline-none" style="transition: all 0.3s ease-in-out;">
            <img src="/imagehome/assets/Maskot Aruteru Shoppu3.svg" alt="Chat CS" class="h-20 sm:h-24 w-auto" />
        </button>
    </div>`;

    function initLayout() {
        document.getElementById('year').textContent = new Date().getFullYear();

        // Mark active nav link
        const path = window.location.pathname;
        document.querySelectorAll('.desktop-nav-link, .mobile-nav-link').forEach(a => {
            const href = a.getAttribute('href') || '';
            if (path === href || (href !== '/' && path.startsWith(href))) {
                a.classList.add('active');
            }
        });

        // Branch Masuk <-> Dashboard berdasarkan login state (localStorage)
        const token = localStorage.getItem('auth_token');
        const authUser = localStorage.getItem('auth_user');
        if (token && authUser) {
            [['desktop-login-link'], ['mobile-login-link']].forEach(([id]) => {
                const link = document.getElementById(id);
                if (link) {
                    link.setAttribute('href', '/account');
                    const span = link.querySelector('span');
                    const icon = link.querySelector('i');
                    if (span) span.textContent = 'Dashboard';
                    if (icon) icon.className = 'bi bi-grid-1x2-fill mr-2';
                }
            });
        }

        // Theme toggle
        const themeToggle = document.getElementById('theme-toggle');
        const darkIcons = document.querySelectorAll('.dark-icon');
        const lightIcons = document.querySelectorAll('.light-icon');
        const savedTheme = localStorage.getItem('theme') || 'dark';
        document.documentElement.setAttribute('data-theme', savedTheme);
        const updateIcons = (theme) => {
            darkIcons.forEach(icon => { icon.style.display = theme === 'dark' ? 'inline-block' : 'none'; });
            lightIcons.forEach(icon => { icon.style.display = theme === 'dark' ? 'none' : 'inline-block'; });
        };
        updateIcons(savedTheme);
        if (themeToggle) {
            themeToggle.addEventListener('click', function () {
                const current = document.documentElement.getAttribute('data-theme');
                const next = current === 'dark' ? 'light' : 'dark';
                document.documentElement.setAttribute('data-theme', next);
                localStorage.setItem('theme', next);
                updateIcons(next);
            });
        }

        // CS chat
        window.toggleCsOptions = function () {
            const options = document.getElementById('cs-options');
            if (options) options.classList.toggle('hidden');
        };
        document.addEventListener('click', function (event) {
            const csOptions = document.getElementById('cs-options');
            const csButton = event.target.closest('button');
            if (csOptions && !csButton && !csOptions.contains(event.target)) {
                csOptions.classList.add('hidden');
            }
        });

        // Popup close
        const dontShow = document.getElementById('dontshowinfoo');
        const popupStructure = document.querySelector('.popup-structuree');
        const closeButtons = document.querySelectorAll('.close-popup-buttonn');
        if (dontShow && popupStructure) {
            function closePopup() {
                if (popupStructure) popupStructure.style.display = 'none';
                if (dontShow && dontShow.checked) localStorage.setItem('hidePopupp', 'true');
            }
            dontShow.addEventListener('change', function () { if (this.checked) closePopup(); });
            closeButtons.forEach(btn => btn.addEventListener('click', closePopup));
        }
    }

    // Expose shared layout data + common Alpine data factory
    window.AruteruLayout = {
        el,
        formatRupiah,
        initLayout,
        navbarHTML,
        searchModalHTML,
        popupHTML,
        footerHTML
    };

    // Load script pendukung bersama (sama dgn footer.php), sekali saja
    function loadScript(src, opts) {
        if (document.querySelector('script[data-src="' + src + '"]')) return;
        const s = document.createElement('script');
        s.src = src;
        if (opts && opts.defer) s.defer = true;
        s.setAttribute('data-src', src);
        if (opts && opts.crossorigin) { s.crossOrigin = opts.crossorigin; s.integrity = opts.integrity; }
        document.body.appendChild(s);
    }

    // react-notif container (target showToast)
    const REACT_NOTIF = '<div id="react-notif"></div>';

    // Auto-mount when page signals it (data-aruteru-layout on <body>)
    document.addEventListener('DOMContentLoaded', function () {
        const body = document.body;
        if (body && body.hasAttribute('data-aruteru-layout')) {
            const headerMount = document.getElementById('site-header');
            const footerMount = document.getElementById('site-footer');
            if (headerMount) headerMount.innerHTML = REACT_NOTIF + navbarHTML + searchModalHTML + popupHTML;
            if (footerMount) footerMount.innerHTML = footerHTML;
            // Script bersama (mirror footer.php)
            loadScript('https://unpkg.com/@lottiefiles/lottie-player@latest/dist/lottie-player.js');
            loadScript('https://cdn.jsdelivr.net/npm/sweetalert2@11');
            loadScript('https://cdn.jsdelivr.net/npm/bootstrap@5.3.3/dist/js/bootstrap.bundle.min.js');
            initLayout();
        }
    });
})();
