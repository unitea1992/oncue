import { useEffect, useId, useRef } from 'react';

interface NativeDialogProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: React.ReactNode;
  labelledBy?: string;
  describedBy?: string;
}

/**
 * native <dialog> ラッパー。Escで閉じる・フォーカス復帰はブラウザ任せ。
 * 保存未完了の間は親がonCloseを無視して開き続ける。
 */
export function NativeDialog({ open, title, onClose, children, labelledBy, describedBy }: NativeDialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const returnFocusRef = useRef<Element | null>(null);
  const autoTitleId = useId();

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      returnFocusRef.current = document.activeElement;
      dialog.showModal();
      dialog.querySelector<HTMLElement>('input:not(:disabled), select:not(:disabled), textarea:not(:disabled), button:not(:disabled)')?.focus();
    } else if (!open && dialog.open) {
      dialog.close();
      // Esc・取消で閉じた場合も開く前のフォーカスへ戻す
      (returnFocusRef.current as HTMLElement | null)?.focus?.();
      returnFocusRef.current = null;
    }
  }, [open]);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    const handleCancel = (event: Event) => {
      event.preventDefault();
      onClose();
    };
    dialog.addEventListener('cancel', handleCancel);
    return () => dialog.removeEventListener('cancel', handleCancel);
  }, [onClose]);

  // 開いたまま外された場合の保険
  useEffect(
    () => () => {
      (returnFocusRef.current as HTMLElement | null)?.focus?.();
    },
    [],
  );

  return (
    <dialog
      ref={ref}
      aria-labelledby={labelledBy ?? autoTitleId}
      aria-describedby={describedBy}
      className="native-dialog"
    >
      <h2 id={labelledBy ?? autoTitleId} className="dialog-title">
        {title}
      </h2>
      {children}
    </dialog>
  );
}

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  tone?: 'default' | 'danger';
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel,
  cancelLabel = 'キャンセル',
  tone = 'default',
  busy = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const messageId = useId();
  return (
    <NativeDialog
      open={open}
      title={title}
      onClose={busy ? () => {} : onCancel}
      describedBy={messageId}
    >
      <p id={messageId} className="dialog-message">
        {message}
      </p>
      <div className="dialog-footer">
        <button type="button" onClick={onCancel} disabled={busy}>
          {cancelLabel}
        </button>
        <button
          type="button"
          className={tone === 'danger' ? 'danger-solid' : 'primary'}
          onClick={onConfirm}
          disabled={busy}
        >
          {busy ? '実行中…' : confirmLabel}
        </button>
      </div>
    </NativeDialog>
  );
}
