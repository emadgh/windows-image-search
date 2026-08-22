from pathlib import Path

settings = Path('src/ui/settings_window.rs')
text = settings.read_text(encoding='utf-8')
text = text.replace('.default_size([900.0, 620.0])', '.default_size([920.0, 640.0])', 1)
text = text.replace('.min_width(700.0)', '.min_width(780.0)', 1)
settings.write_text(text, encoding='utf-8')

collections = Path('src/ui/collections.rs')
text = collections.read_text(encoding='utf-8')
text = text.replace('ui.set_min_width(250.0);', 'ui.set_min_width(200.0);', 1)
text = text.replace('ui.set_min_width(470.0);', 'ui.set_min_width(350.0);', 1)
text = text.replace('.desired_width(240.0),', '.desired_width(180.0),', 1)
collections.write_text(text, encoding='utf-8')
