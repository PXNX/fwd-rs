--drop table accesses;
--drop table links;


CREATE TABLE if NOT exists links
(
    id          INT GENERATED ALWAYS AS IDENTITY,
    author     bigint      NOT NULL,
    target     text        not null,
    title     text        not null,
    primary key (id)
);

CREATE TABLE if NOT exists accesses
(
    id          INT GENERATED ALWAYS AS IDENTITY,
    link_id     INT NOT NULL,
    address text not null,
       accessed_at timestamptz NOT NULL default current_date,
    primary key (id),
        CONSTRAINT linked
        FOREIGN KEY (link_id) REFERENCES links (id)
);

