import { useEffect, useRef, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { SettingsGroup } from "../../../components/FormControls";
import {
  authorSummary,
  loadAcknowledgements,
  spdxIds,
  spdxUrl,
  type Acknowledgement,
} from "./acknowledgements";

export default function AcknowledgementsView({
  onBack,
  onOpenUrl,
  load = loadAcknowledgements,
}: {
  onBack: () => void;
  onOpenUrl: (url: string) => void;
  load?: () => Promise<Acknowledgement[]>;
}) {
  const { t } = useLocale();
  const [items, setItems] = useState<Acknowledgement[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  // The pane replaces the About content in the same scroller, which may be scrolled down.
  useEffect(() => {
    rootRef.current?.scrollIntoView?.({ block: "start" });
  }, []);

  useEffect(() => {
    let active = true;
    load()
      .then((list) => {
        if (active) setItems(list);
      })
      .catch((error) => {
        if (active) setLoadError(String(error));
      });
    return () => {
      active = false;
    };
  }, [load]);

  return (
    <div ref={rootRef} className="about-section acknowledgements">
      <button type="button" className="credential-btn acknowledgements__back" onClick={onBack}>
        <span aria-hidden="true">‹ </span>
        {t("TabAbout")}
      </button>

      <SettingsGroup
        title={t("AboutLinkAcknowledgements")}
        footer={<p className="acknowledgements__footer">{t("AcknowledgementsFooter")}</p>}
      >
        {loadError ? (
          <p className="settings-group__error" role="alert">
            {t("ErrorPrefix")} {loadError}
          </p>
        ) : items === null ? (
          <p className="settings-group__row about-update-msg">{t("AboutLoading")}</p>
        ) : (
          items.map((item) => (
            <AcknowledgementRow
              key={`${item.ecosystem}:${item.name}`}
              item={item}
              onOpenUrl={onOpenUrl}
            />
          ))
        )}
      </SettingsGroup>
    </div>
  );
}

function AcknowledgementRow({
  item,
  onOpenUrl,
}: {
  item: Acknowledgement;
  onOpenUrl: (url: string) => void;
}) {
  const { t } = useLocale();
  // Licence bodies are only mounted while a row is open; hundreds of full texts
  // in the DOM at once make the pane sluggish in WebView2.
  const [open, setOpen] = useState(false);
  const summary = authorSummary(item.authors, t("AcknowledgementsOthers"));

  return (
    <details
      className="acknowledgement"
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="acknowledgement__summary">
        <span className="acknowledgement__name">{item.name}</span>
        {summary && <span className="acknowledgement__authors">{summary}</span>}
        <span className="acknowledgement__license">{item.license}</span>
      </summary>
      {open && (
        <div className="acknowledgement__body">
          {item.authors.length > 0 && (
            <div className="acknowledgement__detail">
              <h4>
                {t(item.authors.length === 1 ? "AcknowledgementsAuthor" : "AcknowledgementsAuthors")}
              </h4>
              {item.authors.map((author) => (
                <p key={author}>{author}</p>
              ))}
            </div>
          )}
          <div className="acknowledgement__detail">
            <h4>{t("AboutLinkWebsite")}</h4>
            <button
              type="button"
              className="about-link about-link--inline acknowledgement__url"
              onClick={() => onOpenUrl(item.url)}
            >
              {item.url}
            </button>
          </div>
          <div className="acknowledgement__detail">
            <h4>{t("AcknowledgementsLicense")}</h4>
            {item.notices.length > 0 ? (
              <>
                {item.notices.some((notice) => notice.standard) && (
                  <p className="acknowledgement__note">{t("AcknowledgementsStandardText")}</p>
                )}
                {item.notices.map((notice, index) => (
                  <pre key={`${notice.file}-${index}`} className="acknowledgement__text" dir="ltr">
                    {notice.text}
                  </pre>
                ))}
              </>
            ) : (
              spdxIds(item.license).map((id) => (
                <button
                  key={id}
                  type="button"
                  className="about-link about-link--inline acknowledgement__url"
                  onClick={() => onOpenUrl(spdxUrl(id))}
                >
                  {spdxUrl(id)}
                </button>
              ))
            )}
          </div>
        </div>
      )}
    </details>
  );
}
