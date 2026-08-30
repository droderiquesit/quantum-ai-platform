const fs = require('fs');
const path = require('path');

const srcDir = path.join(process.cwd(), 'src');
const files = fs.readdirSync(srcDir).filter(f => f.endsWith('.html'));

const mapping = {
  'bg-[#F4F6F9]': 'bg-bg-dark',
  'bg-white': 'bg-surface',
  'border-gray-100': 'border-white/5',
  'border-gray-50': 'border-white/5',
  'text-[#0f172a]': 'text-white',
  'text-[#64748b]': 'text-white/50',
  'text-[#94a3b8]': 'text-white/50',
  'bg-gray-50': 'bg-surface-2',
  'bg-gray-100': 'bg-surface-2',
  'text-[#00C896]': 'text-success',
  'text-[#FFB800]': 'text-warning',
  'text-[#ef4444]': 'text-error',
  '#1e293b': 'currentColor'
};

let updated = 0;

files.forEach(file => {
  const filePath = path.join(srcDir, file);
  let content = fs.readFileSync(filePath, 'utf8');
  let originalContent = content;

  Object.keys(mapping).forEach(key => {
    content = content.split(key).join(mapping[key]);
  });

  if (content !== originalContent) {
    fs.writeFileSync(filePath, content, 'utf8');
    updated++;
    console.log('Updated ' + file);
  }
});

console.log('Total files updated: ' + updated);
