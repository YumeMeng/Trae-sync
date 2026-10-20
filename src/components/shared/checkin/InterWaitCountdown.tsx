import { useEffect, useState } from "react";
import { Loader2, Timer } from "lucide-react";

/**
 * 本地秒级倒计时（后端只在阶段开始时推送一次总时长，前端自行递减）。
 */
export function useCountdownSeconds(initial: number) {
  const [remaining, setRemaining] = useState(initial);
  useEffect(() => {
    setRemaining(initial);
  }, [initial]);
  useEffect(() => {
    if (remaining <= 0) return;
    const timer = window.setTimeout(() => setRemaining((value) => Math.max(0, value - 1)), 1000);
    return () => window.clearTimeout(timer);
  }, [remaining]);
  return remaining;
}

/**
 * 账号间错峰等待倒计时：批量执行时下一账号前的 3~8 秒随机间隔
 * 显示"N 秒后开始"；归零后显示"开始中…"（由 running 阶段接管）。
 */
export function InterWaitCountdown({ seconds }: { seconds: number }) {
  const remaining = useCountdownSeconds(seconds);
  if (remaining <= 0) {
    return (
      <>
        <Loader2 size={13} className="checkin-flow__spinner" aria-hidden="true" />开始中…
      </>
    );
  }
  return (
    <>
      <Timer size={13} aria-hidden="true" />{remaining} 秒后开始
    </>
  );
}
