import React, { useEffect, useRef, useState } from "react";
import { AlertCircle, Check, X } from "lucide-react";

// 隐私模式或被禁用的存储会抛出异常，本地偏好读写失败时退回默认值。
export const store = {
  get(key: string): string | null {
    try {
      return localStorage.getItem("tv-rust:" + key);
    } catch {
      return null;
    }
  },
  set(key: string, value: string) {
    try {
      localStorage.setItem("tv-rust:" + key, value);
    } catch {
      // 忽略：偏好仅在本次页面内有效。
    }
  },
  remove(key: string) {
    try {
      localStorage.removeItem("tv-rust:" + key);
    } catch {
      // 忽略。
    }
  },
};

type ToastItem = { id: number; text: string; kind: "ok" | "error" };
let pushToast: ((text: string, kind: ToastItem["kind"]) => void) | null = null;
let dropToasts: (() => void) | null = null;
export const toast = (text: string, kind: ToastItem["kind"] = "ok") =>
  pushToast?.(text, kind);
export const clearToasts = () => dropToasts?.();

export function Toasts() {
  const [items, setItems] = useState<ToastItem[]>([]);
  useEffect(() => {
    let next = 0;
    const timers = new Set<number>();
    pushToast = (text, kind) => {
      const id = ++next;
      setItems((old) => [...old.slice(-2), { id, text, kind }]);
      const timer = window.setTimeout(
        () => {
          timers.delete(timer);
          setItems((old) => old.filter((item) => item.id !== id));
        },
        kind === "error" ? 6000 : 3600,
      );
      timers.add(timer);
    };
    dropToasts = () => setItems([]);
    return () => {
      pushToast = null;
      dropToasts = null;
      timers.forEach((timer) => clearTimeout(timer));
    };
  }, []);
  return (
    <div className="toasts" role="status" aria-live="polite">
      {items.map((item) => (
        <div key={item.id} className={`toast ${item.kind}`}>
          {item.kind === "ok" ? <Check size={16} /> : <AlertCircle size={16} />}
          <span>{item.text}</span>
        </div>
      ))}
    </div>
  );
}

let openModals = 0;
export const modalOpen = () => openModals > 0;

export function Modal({
  onClose,
  locked = false,
  closeLabel = "关闭",
  children,
}: {
  onClose: () => void;
  locked?: boolean;
  closeLabel?: string;
  children: React.ReactNode;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  const state = useRef({ onClose, locked });
  state.current = { onClose, locked };
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const overflow = document.body.style.overflow;
    if (++openModals === 1) document.body.style.overflow = "hidden";
    dialog.current?.focus({ preventScroll: true });
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !state.current.locked) {
        event.stopPropagation();
        state.current.onClose();
      }
    };
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("keydown", key);
      if (--openModals === 0) document.body.style.overflow = overflow;
      previous?.focus?.({ preventScroll: true });
    };
  }, []);
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        // 只在遮罩上按下时关闭，避免在弹窗内选择文字后松开鼠标误关。
        if (event.target === event.currentTarget && !locked) onClose();
      }}
    >
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        ref={dialog}
      >
        <button
          className="close"
          type="button"
          disabled={locked}
          onClick={onClose}
          aria-label={closeLabel}
        >
          <X />
        </button>
        {children}
      </div>
    </div>
  );
}

export function Confirm({
  title,
  children,
  action,
  busy = false,
  onConfirm,
  onClose,
}: {
  title: string;
  children: React.ReactNode;
  action: string;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Modal onClose={onClose} locked={busy}>
      <h2>{title}</h2>
      <p>{children}</p>
      <div className="confirm-actions">
        <button className="outline" disabled={busy} onClick={onClose}>
          取消
        </button>
        <button className="danger" disabled={busy} onClick={onConfirm}>
          {action}
        </button>
      </div>
    </Modal>
  );
}

export function SkeletonCards({ count = 6 }: { count?: number }) {
  return (
    <div className="cards" aria-busy="true" aria-label="正在加载">
      {Array.from({ length: count }, (_, index) => (
        // 占位不使用 card/poster 类名，避免被当作真实条目。
        <div className="skeleton" key={index}>
          <div className="skeleton-poster" />
          <i />
          <i />
        </div>
      ))}
    </div>
  );
}
