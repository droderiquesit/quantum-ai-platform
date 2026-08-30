const fs = require('fs');
const path = require('path');

const scriptStr = `
    <!-- Theme Script (Placed early to prevent FOUC) -->
    <script>
      (function() {
        const saved = localStorage.getItem('theme') || 'dark';
        if (saved === 'dark') document.documentElement.classList.add('dark');
        else document.documentElement.classList.remove('dark');
      })();
    </script>
  </head>`;

const srcDir = path.join(process.cwd(), 'src');
const files = fs.readdirSync(srcDir).filter(f => f.endsWith('.html'));

let fixedCount = 0;

files.forEach(file => {
  const filePath = path.join(srcDir, file);
  let content = fs.readFileSync(filePath, 'utf8');
  
  // Clean up if it was previously injected badly
  content = content.replace(/    <script>\n      \(function\(\)\{\n        const saved = localStorage\.getItem\("theme"\) \|\| "dark";\n        if \(saved === "dark"\) document\.documentElement\.classList\.add\("dark"\);\n        else document\.documentElement\.classList\.remove\("dark"\);\n      \}\)\(\);\n    <\/script>\n  <\/head>/g, '</head>');
  
  // Inject cleanly if not present
  if (!content.includes('localStorage.getItem')) {
    content = content.replace('</head>', scriptStr);
    fs.writeFileSync(filePath, content, 'utf8');
    fixedCount++;
    console.log('Fixed FOUC on ' + file);
  }
});

console.log('Total files fixed: ' + fixedCount);
