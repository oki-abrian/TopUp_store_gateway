$(document).ready(function () {
    $('#darkl').toggleClass(localStorage.toggled);
    $('.nav-link-style').on('click', function () {
        darkLight()
    });
    if ($('#darkl').hasClass('dark-layout')) {
        $('#moonsun').removeClass('icon-moon');
        $('#moonsun').addClass('icon-sun');
        $('.main-menu').removeClass('menu-light').addClass('menu-dark');
        $('.header-navbar').removeClass('navbar-light').addClass('navbar-dark');
    } else {
        $('#moonsun').addClass('icon-moon');
        $('#moonsun').removeClass('icon-sun');
        $('.main-menu').removeClass('menu-dark').addClass('menu-light');
        $('.header-navbar').removeClass('navbar-dark').addClass('navbar-light');
    }
});

function darkLight() {
    if (localStorage.toggled != 'dark-layout') {
        $('#darkl, p').toggleClass('dark-layout', true);
        localStorage.toggled = "dark-layout";
        $('#moonsun').removeClass('icon-moon');
        $('#moonsun').addClass('icon-sun');
        $('.main-menu').removeClass('menu-light').addClass('menu-dark');
        $('.header-navbar').removeClass('navbar-light').addClass('navbar-dark');
    } else {
        $('#darkl, p').toggleClass('dark-layout', false);
        localStorage.toggled = "";
        $('#moonsun').addClass('icon-moon');
        $('#moonsun').removeClass('icon-sun');
        $('.main-menu').removeClass('menu-dark').addClass('menu-light');
        $('.header-navbar').removeClass('navbar-dark').addClass('navbar-light');
    }
}