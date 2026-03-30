import { useEffect, useState } from "react";

export type Theme = "dark" | "light" | "auto";

const STORAGE_KEY = "camdl-theme";

export function applyTheme(theme: Theme) {
  document.documentElement.setAttribute("data-theme", theme);
  // Keep Tailwind's `dark` class in sync for `dark:` utility variants
  const isDark = theme === "dark"
    || (theme === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", isDark);
}

export function getInitialTheme(): Theme {
  return (localStorage.getItem(STORAGE_KEY) as Theme) ?? "auto";
}

export function useIsDark(): boolean {
  const [isDark, setIsDark] = useState(() => document.documentElement.classList.contains("dark"));
  useEffect(() => {
    const obs = new MutationObserver(() => setIsDark(document.documentElement.classList.contains("dark")));
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => obs.disconnect();
  }, []);
  return isDark;
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(getInitialTheme);

  useEffect(() => {
    applyTheme(theme);
    localStorage.setItem(STORAGE_KEY, theme);

    if (theme === "auto") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const handler = () => applyTheme("auto");
      mq.addEventListener("change", handler);
      return () => mq.removeEventListener("change", handler);
    }
  }, [theme]);

  const cycle = () => setThemeState((t) => t === "dark" ? "light" : t === "light" ? "auto" : "dark");

  return { theme, setTheme: setThemeState, cycle };
}
