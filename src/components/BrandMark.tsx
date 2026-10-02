/** OnCue のマーク。Q の字とキューライトのランプを重ねた形。 */
export function BrandMark({ className = 'brand-mark' }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 64 64" aria-hidden focusable="false">
      <rect width="64" height="64" rx="15" fill="#8C1D40" />
      <circle cx="31" cy="29" r="17" fill="none" stroke="#FFFFFF" strokeWidth="6" />
      <circle cx="31" cy="29" r="8" fill="#F6B544" />
      <path d="M37 41 L50 52" stroke="#FFFFFF" strokeWidth="7" strokeLinecap="round" />
    </svg>
  );
}
