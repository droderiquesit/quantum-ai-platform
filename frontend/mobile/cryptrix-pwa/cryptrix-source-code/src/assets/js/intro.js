let current = 0;
const total = 3;

function nextSlide() {
  const slides = document.querySelectorAll(".slide");
  const dots = document.querySelectorAll('[id^="dot-"]');
  const btn = document.getElementById("next-btn");

  if (current < total - 1) {
    slides[current].classList.add("-translate-x-full");
    slides[current].classList.remove("translate-x-0");
    current++;
    slides[current].classList.remove("translate-x-full");
    slides[current].classList.add("translate-x-0");
    // Update dots
    dots.forEach((d, i) => {
      d.className =
        i === current
          ? "w-6 h-1.5 rounded-full bg-primary transition-all"
          : "w-1.5 h-1.5 rounded-full bg-white/20 transition-all";
    });
    if (current === total - 1) {
      btn.innerHTML =
        'Get Started <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14M12 5l7 7-7 7"/></svg>';
    }
  } else {
    window.location.href = "signup.html";
  }
}

// Initialize first slide visible
document.getElementById("slide-0").classList.add("translate-x-0");
