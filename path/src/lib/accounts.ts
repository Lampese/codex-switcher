// complete code
import fs from 'fs';
import path from 'path';

interface Account {
  id: string;
  name: string;
  token: string;
}

class Accounts {
  private accountsPath: string;

  constructor() {
    this.accountsPath = path.join(process.env.HOME, '.codex-switcher', 'accounts.json');
  }

  async loadAccounts(): Promise<Account[]> {
    try {
      const data = await fs.promises.readFile(this.accountsPath, 'utf8');
      return JSON.parse(data);
    } catch (err) {
      if (err.code === 'ENOENT') {
        // Handle the case where the file does not exist
        return [];
      } else {
        throw err;
      }
    }
  }

  async saveAccounts(accounts: Account[]): Promise<void> {
    const accountsJson = JSON.stringify(accounts);
    await fs.promises.writeFile(this.accountsPath, accountsJson);
  }
}

// Example usage:
const accounts = new Accounts();
accounts.loadAccounts().then((accounts) => {
  console.log(accounts);
  accounts.saveAccounts(accounts).then(() => {
    console.log('Accounts updated');
  }).catch((err) => {
    console.error('Error updating accounts:', err);
  });
});