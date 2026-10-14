import { useEffect, useState } from "react";
import { fetchAppInfo } from "../lib/tauri";

/** True on Android (Tauri appInfo platform contains "android"). Defaults to false. */
export function useIsAndroid(): boolean {
  const [isAndroid, setIsAndroid] = useState(false);
  useEffect(() => {
    let cancelled = false;
    fetchAppInfo()
      .then((info) => {
        if (!cancelled) setIsAndroid(info.platform.toLowerCase().includes("android"));
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);
  return isAndroid;
}
