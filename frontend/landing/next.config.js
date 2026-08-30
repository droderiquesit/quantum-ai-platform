/** @type {import('next').NextConfig} */
// Cloud Run serves the landing from Next's standalone output: server.js
// plus only the traced node_modules, in a container a fraction of the size
// of the full install.
const nextConfig = { output: "standalone" }

module.exports = nextConfig
