import { useState } from 'react';
import { version as buildVersion } from '../../package.json';
import appIcon from '../../assets/app-icon.svg';
import { t } from '../i18n';
import { desktop } from '../platform/client';
import { Icon } from './Icon';

const projectUrl = 'https://github.com/UClipbot/UClip';
const projectLinks = [
  { page: 'releases', label: 'about_releases', path: '/releases', icon: 'history' },
  { page: 'source', label: 'about_source', path: '', icon: 'link' },
  { page: 'issues', label: 'about_issues', path: '/issues', icon: 'info' },
] as const;

export function AboutSettings({ native, version }: { native: boolean; version?: string }) {
  const [linkError, setLinkError] = useState(false);

  async function openPage(page: (typeof projectLinks)[number]['page']) {
    setLinkError(false);
    try {
      await desktop.openProjectPage(page);
    } catch {
      setLinkError(true);
    }
  }

  return (
    <div className="about-content">
      <header className="about-identity">
        <img src={appIcon} width={64} height={64} alt="" />
        <div>
          <h3>UClip</h3>
          <p className="about-version">
            {t('about_version', { version: version ?? buildVersion })}
          </p>
          <p className="about-description">
            {t(
              native
                ? 'about_description'
                : 'this_is_a_browser_preview_all_records_are_in_memory_samples',
            )}
          </p>
        </div>
      </header>
      <ul className="about-links">
        {projectLinks.map(({ page, label, path, icon }) => {
          const content = (
            <>
              <Icon name={icon} size={17} />
              <span>{t(label)}</span>
              <Icon name="external" size={14} />
            </>
          );
          return (
            <li key={page}>
              {native ? (
                <button type="button" onClick={() => void openPage(page)}>
                  {content}
                </button>
              ) : (
                <a href={projectUrl + path} target="_blank" rel="noreferrer">
                  {content}
                </a>
              )}
            </li>
          );
        })}
      </ul>
      {linkError && (
        <p className="settings-feedback" role="alert">
          {t('about_open_failed')}
        </p>
      )}
    </div>
  );
}
