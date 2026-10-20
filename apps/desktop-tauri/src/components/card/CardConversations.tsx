import { useState } from "react";
import type { CardConversations as CardConversationsModel } from "../../lib/cardConversations";

/**
 * The card's collapsible Conversations list, the Windows form of upstream's
 * Cost submenu list (`CostHistoryChartMenuView.sessionsBlock`): title, usage
 * line and last activity on the left, the estimated cost on the right.
 */
export default function CardConversations({
  model,
  onToggle,
}: {
  model: CardConversationsModel;
  onToggle?: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <section className="card-conversations">
      <div className="card-conversations__header">
        <button
          type="button"
          className="card-conversations__toggle"
          aria-expanded={expanded}
          onClick={() => {
            setExpanded((value) => !value);
            onToggle?.();
          }}
        >
          {model.title}
        </button>
        <span className="card-conversations__count">{model.count}</span>
      </div>
      {expanded && (
        <ul className="card-conversations__list">
          {model.rows.map((row) => (
            <li className="card-conversations__row" key={row.id} title={row.sessionLabel}>
              <div className="card-conversations__text">
                <span className="card-conversations__title">{row.title}</span>
                <span className="card-conversations__meta">{row.usageLine}</span>
                {row.lastActivity && (
                  <span className="card-conversations__meta">{row.lastActivity}</span>
                )}
              </div>
              <span className="card-conversations__cost">{row.cost}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
