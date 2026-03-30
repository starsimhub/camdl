/// <reference types="vite/client" />

// Allow ?raw imports of any file extension
declare module "*?raw" {
  const content: string;
  export default content;
}
