import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function baseProps(size = 16, props: IconProps) {
  const { size: _s, ...rest } = props;
  return {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.65,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
    ...rest,
  };
}

export function BrandMark({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="1.5" y="1.5" width="21" height="21" rx="5.4" fill="#1A1A1D" />
      <path
        d="M7.7 17.6V6.6"
        stroke="#6C6C77"
        strokeWidth="1.75"
        strokeLinecap="round"
      />
      <path
        d="M7.7 17.6V14.9C7.7 11.7 15.3 12.5 15.3 9.3V6.9"
        stroke="#FFFFFF"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M12.5 9.4L15.3 6.6L18.1 9.4"
        stroke="#FFFFFF"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function ActivityIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M3 12h4l2.5-6 5 12 2.5-6H21" />
    </svg>
  );
}

export function AppsIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <rect x="3.5" y="3.5" width="7" height="7" rx="2" />
      <rect x="13.5" y="3.5" width="7" height="7" rx="2" />
      <rect x="3.5" y="13.5" width="7" height="7" rx="2" />
      <rect x="13.5" y="13.5" width="7" height="7" rx="2" />
    </svg>
  );
}

export function RouteIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <circle cx="6" cy="6" r="2.5" />
      <circle cx="18" cy="18" r="2.5" />
      <path d="M8.5 6H14a3 3 0 0 1 3 3v1.5M15.5 18H10a3 3 0 0 1-3-3v-1.5" />
      <circle cx="18" cy="6" r="2" />
    </svg>
  );
}

export function ShieldIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M12 3l7 3v5.5c0 4.6-2.9 8.1-7 9.5-4.1-1.4-7-4.9-7-9.5V6l7-3z" />
      <path d="M9.5 12.2l1.8 1.8 3.6-3.8" />
    </svg>
  );
}

export function PlayIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <polygon points="7 5 19 12 7 19 7 5" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function StopIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function SearchIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <circle cx="11" cy="11" r="6.5" />
      <path d="M16 16l4.2 4.2" />
    </svg>
  );
}

export function RefreshIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M20 11a8 8 0 1 0 2 5.3" />
      <path d="M20 4v7h-7" />
    </svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M7 7l1 12a1 1 0 0 0 1 1h6a1 1 0 0 0 1-1l1-12" />
    </svg>
  );
}

export function SettingsIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.2a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.2a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.2a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9c.3.7.9 1.1 1.5 1.1H21a2 2 0 1 1 0 4h-.2a1.7 1.7 0 0 0-1.4.9z" />
    </svg>
  );
}

export function TerminalIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M7.5 10l2.5 2-2.5 2M12.5 14h4" />
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M5 12.5l4.5 4.5L19 7.5" />
    </svg>
  );
}

export function PauseIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <rect x="6.5" y="5.5" width="3.5" height="13" rx="1" />
      <rect x="14" y="5.5" width="3.5" height="13" rx="1" />
    </svg>
  );
}

export function LanguageIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M4 6h9M8.5 4v2M11 6c-.8 3.2-2.8 6-6 7.5M6.5 9c1.2 1.8 2.8 3.2 4.8 4.2" />
      <path d="M13.5 19l3-8 3 8M14.6 16.2h3.8" />
    </svg>
  );
}

export function MinimizeIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M5 12h14" strokeWidth="1.5" />
    </svg>
  );
}

export function MaximizeIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <rect x="5.5" y="5.5" width="13" height="13" rx="2" strokeWidth="1.5" />
    </svg>
  );
}

export function RestoreIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <rect x="5" y="8" width="11" height="11" rx="1.8" strokeWidth="1.5" />
      <path d="M8.5 8V6.2A1.7 1.7 0 0 1 10.2 4.5h7.6a1.7 1.7 0 0 1 1.7 1.7v7.6a1.7 1.7 0 0 1-1.7 1.7H16" strokeWidth="1.5" />
    </svg>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M6 6l12 12M18 6L6 18" strokeWidth="1.55" />
    </svg>
  );
}

export function BanIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M6.2 6.2l11.6 11.6" />
    </svg>
  );
}

export function GlobeIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M3.8 12h16.4M12 3.5c2.4 2.4 3.6 5.2 3.6 8.5s-1.2 6.1-3.6 8.5c-2.4-2.4-3.6-5.2-3.6-8.5S9.6 5.9 12 3.5z" />
    </svg>
  );
}

export function SortAscIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M8 15l4-5 4 5" strokeWidth="1.75" />
    </svg>
  );
}

export function SortDescIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M8 9l4 5 4-5" strokeWidth="1.75" />
    </svg>
  );
}

export function SortNeutralIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M8.5 9.5L12 6l3.5 3.5M8.5 14.5L12 18l3.5-3.5" strokeWidth="1.5" />
    </svg>
  );
}

export function ChevronLeftIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M14.5 6.5L9 12l5.5 5.5" />
    </svg>
  );
}

export function ChevronRightIcon(props: IconProps) {
  return (
    <svg {...baseProps(props.size, props)}>
      <path d="M9.5 6.5L15 12l-5.5 5.5" />
    </svg>
  );
}
