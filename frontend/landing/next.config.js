/** @type {import('next').NextConfig} */
// Cloud Run serves the landing from Next's standalone output: server.js
// plus only the traced node_modules, in a container a fraction of the size
// of the full install.
const nextConfig = {
    output: "standalone",

    async redirects() {
        return [
            // /about and /company were two pages saying overlapping things about
            // one company, which is how two pages come to disagree. /company is
            // the one that exists; /about keeps working for anything already
            // linking to it.
            { source: "/about", destination: "/company", permanent: true },
            // /error was a routable page that returned HTTP 200 while displaying
            // "404". It is an error boundary now. Anything holding the old URL
            // lands somewhere real instead of on a lie about its own status.
            { source: "/error", destination: "/", permanent: true },
        ]
    },

    async headers() {
        return [
            {
                source: "/:path*",
                headers: [
                    // The public site renders no platform data and needs no
                    // framing, no plugins and no cross-origin embedding. These are
                    // the cheapest controls on the site and the only ones a
                    // visitor's browser can enforce for us.
                    { key: "X-Content-Type-Options", value: "nosniff" },
                    { key: "X-Frame-Options", value: "DENY" },
                    { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
                    { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=(), payment=()" },
                ],
            },
        ]
    },
}

module.exports = nextConfig
