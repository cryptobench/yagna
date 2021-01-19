-- HACK: All this code below is just to drop column network from table gnt_driver_transaction

PRAGMA foreign_keys=off;

CREATE TABLE gnt_driver_transaction_tmp
(
	tx_id VARCHAR(128) NOT NULL PRIMARY KEY,
	sender VARCHAR(40) NOT NULL,
	-- U256 in big endian hex
	nonce VARCHAR(64) NOT NULL,
	timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
	status INTEGER NOT NULL,
	tx_type INTEGER NOT NULL,
	encoded VARCHAR (8000) NOT NULL,
	signature VARCHAR (130) NOT NULL,
	tx_hash VARCHAR(64),
	FOREIGN KEY(status) REFERENCES gnt_driver_transaction_status (status_id),
	FOREIGN KEY(tx_type) REFERENCES gnt_driver_transaction_type (type_id)
);

INSERT INTO gnt_driver_transaction_tmp(tx_id, sender, nonce, timestamp, status, tx_type, encoded, signature, tx_hash)
SELECT tx_id, sender, nonce, timestamp, status, tx_type, encoded, signature, tx_hash FROM gnt_driver_transaction;


DROP TABLE gnt_driver_transaction;

ALTER TABLE gnt_driver_transaction_tmp RENAME TO gnt_driver_transaction;

PRAGMA foreign_keys=on;
