const fs = require('fs');
const path = require('path');

const srcDir = path.join(process.cwd(), 'src');
const files = fs.readdirSync(srcDir).filter(f => f.endsWith('.html'));

files.forEach(file => {
  const filePath = path.join(srcDir, file);
  let content = fs.readFileSync(filePath, 'utf8');

  let modified = false;

  if (!content.includes('app.js')) {
    // If it's a page that uses data.js or charts.js, inject app.js and components.js
    if (content.includes('</body>')) {
        content = content.replace('</body>', '  <script src="./assets/js/components.js"></script>\n    <script src="./assets/js/app.js"></script>\n  </body>');
        fs.writeFileSync(filePath, content, 'utf8');
        console.log('Fixed missing app.js in: ' + file);
    }
  }
});
