import React, { useEffect, useRef } from "react";

export interface ConfirmSpec {
  title: string;
  body: React.ReactNode;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel?: () => void;
}

export default function ConfirmDialog({ spec }: { spec: ConfirmSpec }) {
  const boxRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    boxRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") spec.onCancel?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [spec]);

  return (
    <div className="modal-backdrop" role="presentation">
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={spec.title}
        tabIndex={-1}
        ref={boxRef}
      >
        <h2>{spec.title}</h2>
        <div style={{ color: "var(--text-secondary)", whiteSpace: "pre-line" }}>{spec.body}</div>
        <div className="modal-actions">
          <button className="btn" onClick={spec.onCancel ?? (() => {})}>
            Cancel
          </button>
          <button className={`btn ${spec.danger ? "danger" : "primary"}`} onClick={spec.onConfirm}>
            {spec.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
