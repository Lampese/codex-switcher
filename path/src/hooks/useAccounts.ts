// complete code
import fs from 'fs';
import path from 'path';
import { v4 as uuidv4 } from 'uuid';

interface Account {
  id: string;
  name: string;
  token: string;
}

const accountsPath = path.join(process.env.HOME, '.codex-switcher', 'accounts.json');

function loadAccounts(): Promise<Account[]> {
  return new Promise((resolve, reject) => {
    fs.readFile(accountsPath, 'utf8', (err, data) => {
      if (err) {
        if (err.code === 'ENOENT') {
          // Handle the case where the file does not exist
          resolve([]);
        } else {
          reject(err);
        }
      } else {
        try {
          const accounts = JSON.parse(data);
          resolve(accounts);
        } catch (e) {
          // Handle the case where the file is not a valid JSON
          console.error(`Error parsing accounts file: ${e}`);
          resolve([]);
        }
      }
    });
  });
}

function saveAccounts(accounts: Account[]): Promise<void> {
  return new Promise((resolve, reject) => {
    const accountsJson = JSON.stringify(accounts);
    fs.writeFile(accountsPath, accountsJson, (err) => {
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    });
  });
}

function getAccounts(): Promise<Account[]> {
  return loadAccounts();
}

function updateAccounts(accounts: Account[]): Promise<void> {
  return saveAccounts(accounts);
}

// Example usage:
getAccounts().then((accounts) => {
  console.log(accounts);
  updateAccounts(accounts).then(() => {
    console.log('Accounts updated');
  }).catch((err) => {
    console.error('Error updating accounts:', err);
  });
});