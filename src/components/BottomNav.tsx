import { IconAgents, IconList, IconRooms } from "./Icon";

/** Single-pane selector on narrow screens. */
export type MobilePane = "chats" | "conversation";

/** Bottom-nav section on narrow screens. "chats" covers list + conversation. */
export type MobileNav = "chats" | "rooms" | "agents";

interface BottomNavProps {
  active: MobileNav;
  onSelect: (nav: MobileNav) => void;
}

const ITEMS: Array<{
  id: MobileNav;
  label: string;
  Icon: typeof IconList;
}> = [
  { id: "chats", label: "Chats", Icon: IconList },
  { id: "rooms", label: "Rooms", Icon: IconRooms },
  { id: "agents", label: "Agents", Icon: IconAgents },
];

export default function BottomNav({ active, onSelect }: BottomNavProps) {
  return (
    <nav className="bottom-nav" aria-label="Main">
      {ITEMS.map(({ id, label, Icon }) => (
        <button
          key={id}
          type="button"
          className="bottom-nav__btn"
          aria-current={active === id ? "page" : undefined}
          onClick={() => onSelect(id)}
        >
          <Icon className="bottom-nav__icon" />
          <span className="bottom-nav__label">{label}</span>
        </button>
      ))}
    </nav>
  );
}
