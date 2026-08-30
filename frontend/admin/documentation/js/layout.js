$(document).ready(function () {
  var AFFIX_TOP_LIMIT = 50;
  var AFFIX_OFFSET = 100;

  // Mobile Menu Toggle — always present; CSS shows it only below 960px.
  var $nav = $(".nav");
  var $toggle = $('<button id="menu-toggle" class="menu-toggle-btn" aria-label="Toggle menu">&#9776;</button>');
  var $backdrop = $('<div class="nav-backdrop"></div>');
  $("body").append($toggle).append($backdrop);

  function openNav() {
    $nav.addClass("open");
    $backdrop.addClass("show");
    $toggle.html("&times;");
  }
  function closeNav() {
    $nav.removeClass("open");
    $backdrop.removeClass("show");
    $toggle.html("&#9776;");
  }

  $toggle.on("click", function () {
    if ($nav.hasClass("open")) closeNav();
    else openNav();
  });
  $backdrop.on("click", closeNav);

  // Close the menu after tapping a nav link (mobile only).
  $(".docs-nav a").on("click", function () {
    if ($(window).width() <= 960) closeNav();
  });

  // Reset menu state when resizing back to desktop.
  $(window).on("resize", function () {
    if ($(window).width() > 960) closeNav();
  });

  // Scrollspy logic
  $(".docs-nav").each(function () {
    var $affixNav = $(this);
    var current = null;
    var $links = $affixNav.find("a");

    function getClosestHeader(top) {
      var last = $links.first();
      for (var i = 0; i < $links.length; i++) {
        var $link = $links.eq(i);
        var href = $link.attr("href");
        if (href.charAt(0) === "#" && href.length > 1) {
          var $anchor = $(href).first();
          if ($anchor.length > 0) {
            var offset = $anchor.offset();
            if (top < offset.top - AFFIX_OFFSET) {
              return last;
            }
            last = $link;
          }
        }
      }
      return last;
    }

    $(window).on("scroll", function () {
      var top = window.scrollY;
      var $current = getClosestHeader(top);

      if (current !== $current) {
        $affixNav.find(".active").removeClass("active");
        $current.addClass("active");
        current = $current;
      }
    });
  });

  prettyPrint();

  // Add Copy Button to Pre tags
  $('pre').each(function() {
    var $pre = $(this);
    var $copyBtn = $('<button class="copy-btn">Copy</button>');
    $pre.append($copyBtn);

    $copyBtn.on('click', function() {
        var $btn = $(this);
        var code = $pre.text().replace('Copy', '').trim();
        navigator.clipboard.writeText(code).then(function() {
            $btn.text('Copied!').addClass('copied');
            setTimeout(function() {
                $btn.text('Copy').removeClass('copied');
            }, 2000);
        });
    });
  });
});

// Footer Year
if(document.getElementById("date")) {
    document.getElementById("date").innerText = new Date().getFullYear();
}
