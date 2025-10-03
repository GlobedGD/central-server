#!/bin/bash
# Generates database models for all modules.
# This should be reran if any new migrations are added (run AFTER building and running the server once, so that the database is updated)

rm -rf src/users/database/entities
DATABASE_URL=sqlite://db.sqlite sea-orm-cli generate entity -o src/users/database/entities
DATABASE_URL=sqlite://features.sqlite sea-orm-cli generate entity -o src/features/database/entities