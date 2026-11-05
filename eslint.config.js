import js from '@eslint/js';
import globals from 'globals';

export default [
  { ignores: ['target/', 'gen/', 'ffmpeg/', 'node_modules/'] },
  js.configs.recommended,
  { files: ['ui/**/*.js'], languageOptions: { globals: globals.browser } },
  { files: ['tests/**/*.js', '*.config.js'], languageOptions: { globals: globals.node } },
];
