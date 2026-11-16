-- Sub-categories: a category can sit under a main category (one level).
ALTER TABLE categories ADD COLUMN parent_id INTEGER REFERENCES categories (id);
