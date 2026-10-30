-- v0.8.2: kitchen teams (stations). Each menu item belongs to at most one
-- team, and a kitchen ticket is printed per team with the team's name as a
-- big header. Items without a team print on a plain general ticket.
CREATE TABLE teams (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    name_alt   TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    active     INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX teams_name ON teams (name COLLATE NOCASE) WHERE active = 1;
INSERT INTO teams (name, name_alt, sort_order) VALUES
    ('Starter', 'ของเรียกน้ำย่อย', 1), ('Main', 'จานหลัก', 2), ('Curry', 'แกง', 3), ('Rice', 'ข้าว', 4);
ALTER TABLE products ADD COLUMN team_id INTEGER REFERENCES teams (id);
-- Default team for new items in a category.
ALTER TABLE categories ADD COLUMN team_id INTEGER REFERENCES teams (id);
