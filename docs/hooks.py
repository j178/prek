import re

def on_config(config, **kwargs):
    """Read raw `site_url` from mkdocs.yml and store in `config._prek_docs`.

    This is needed because Mkdocs https://prek.j178.dev to http://127.0.0.1:8000
    when serving locally.
    """

    mkdocs_file_path = config['config_file_path']
    with open(mkdocs_file_path, 'r') as f:
        for line in f:
            if line.startswith('site_url:'):
                config._prek_docs = {
                    'site_url': line.split('site_url:')[1].strip()
                }
                break
    return config

def on_page_markdown(markdown, **kwargs):
    """Convert absolute URLs like https://prek.j178.dev/installation into
    relative paths like installation.md for local serving to avoid duplication
    of content.
    """
    site_url = kwargs['config']._prek_docs['site_url']

    def replacement(match):
        url = match.group(0)
        path = url.replace(site_url, '', 1)

        # Strip any query string and separate fragment identifiers.
        path = path.split('?', 1)[0]
        path, *fragment = path.split('#', 1)

        path = path.lstrip('/').rstrip('/')
        if not path:
            path = 'index'

        last_segment = path.rsplit('/', 1)[-1]
        if '.' not in last_segment:
            path = f"{path}.md"

        if fragment:
            return f"{path}#{fragment[0]}"
        return path

    return re.sub(rf'{re.escape(site_url)}([^)\s]+)', replacement, markdown)
