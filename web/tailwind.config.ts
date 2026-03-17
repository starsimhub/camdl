import type { Config } from 'tailwindcss';

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      fontFamily: {
        mono: ['JetBrains Mono', 'Fira Code', 'Menlo', 'monospace'],
      },
      colors: {
        surface: {
          0: '#0f1117',
          1: '#161b22',
          2: '#1c2128',
          3: '#22272e',
          border: '#30363d',
        },
        accent: {
          DEFAULT: '#2dd4bf',
          dim: '#1a9e8e',
        },
      },
    },
  },
  plugins: [],
} satisfies Config;
