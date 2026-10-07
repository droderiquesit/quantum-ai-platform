#!/usr/bin/env node

/**
 * Bundle analysis script: generates bundle reports for performance analysis.
 * Run: npm run analyze-bundle or ANALYZE=true npm run build
 *
 * Reports generated in .next/
 */

import fs from "fs";
import path from "path";

const nextDir = path.join(process.cwd(), ".next");

if (!fs.existsSync(nextDir)) {
  console.error(
    "Error: .next directory not found. Run 'npm run build' first."
  );
  process.exit(1);
}

// Analyze chunk sizes from .next directory
function analyzeChunks() {
  const chunksDir = path.join(nextDir, "static", "chunks");

  if (!fs.existsSync(chunksDir)) {
    console.warn("No chunks found. Build may have failed.");
    return;
  }

  const chunks = fs.readdirSync(chunksDir);
  const files = chunks
    .map((file) => {
      const filePath = path.join(chunksDir, file);
      const size = fs.statSync(filePath).size;
      return { file, size };
    })
    .sort((a, b) => b.size - a.size);

  console.log("\n📦 Bundle Chunk Analysis (Top 10 largest):");
  console.log("─".repeat(60));
  files.slice(0, 10).forEach((f, i) => {
    const sizeKB = (f.size / 1024).toFixed(2);
    const sizeWarning = f.size > 200000 ? " ⚠️" : "";
    console.log(`${i + 1}. ${f.file.padEnd(40)} ${sizeKB.padStart(10)} KB${sizeWarning}`);
  });

  const totalSize = files.reduce((sum, f) => sum + f.size, 0);
  console.log("─".repeat(60));
  console.log(`Total bundle size: ${(totalSize / 1024).toFixed(2)} KB`);

  // Performance targets
  const maxMainChunk = 200000; // 200 KB
  const maxTotalBundle = 1000000; // 1 MB
  let issues = 0;

  const mainChunk = files.find((f) => f.file.includes("main"));
  if (mainChunk && mainChunk.size > maxMainChunk) {
    console.log(
      `\n❌ Main chunk exceeds target (${(mainChunk.size / 1024).toFixed(2)} KB > 200 KB)`
    );
    issues++;
  }

  if (totalSize > maxTotalBundle) {
    console.log(
      `\n❌ Total bundle exceeds target (${(totalSize / 1024).toFixed(2)} KB > 1000 KB)`
    );
    issues++;
  }

  if (issues === 0) {
    console.log("\n✅ Bundle size targets met");
  }

  return files;
}

// Analyze page-specific chunks
function analyzePageChunks() {
  const pagesDir = path.join(nextDir, "static", "chunks", "app");

  if (fs.existsSync(pagesDir)) {
    console.log("\n📄 Page-Specific Chunks:");
    console.log("─".repeat(60));

    const pageChunks = fs.readdirSync(pagesDir);
    pageChunks.forEach((file) => {
      const filePath = path.join(pagesDir, file);
      const size = fs.statSync(filePath).size;
      const sizeKB = (size / 1024).toFixed(2);
      console.log(`${file.padEnd(40)} ${sizeKB.padStart(10)} KB`);
    });
  }
}

// Generate performance metrics
function generateReport() {
  console.log("\n📊 Performance Report");
  console.log("═".repeat(60));

  analyzeChunks();
  analyzePageChunks();

  console.log("\n💡 Recommendations:");
  console.log("  • Use dynamic imports for route-specific code");
  console.log("  • Implement lazy loading for images and components");
  console.log("  • Check for unused dependencies with 'npm ls'");
  console.log("  • Monitor chunk sizes in CI/CD pipeline");
  console.log("═".repeat(60));
}

generateReport();
